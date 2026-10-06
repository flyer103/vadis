//! setup/anchor.rs — the line-oriented locator (DESIGN §12.14's anchor
//! grammar; spec §4.11's write strategy).
//!
//! A **line locator, not a YAML parser**: it never builds a document and
//! never rewrites one — a document would discard the comments the whole
//! strategy exists to protect. It maps a key path to a unique
//! `(line, byte range within that line)`, or refuses.
//!
//! Grammar (frozen):
//!   `a.b.c`                    key `c` in block `b` in the document's `a` block
//!   `a.b[i]`                   the i-th entry of the block **sequence** `b`,
//!                              when that entry is a single-line scalar
//!   `a[id=NAME].k`             key `k` inside the list entry whose own
//!   `providers[name=NAME].k`   sibling `id:` / `name:` is `NAME`
//!
//! Locator rules (DESIGN §12.14, "every rule is stated"):
//!   1. indentation is spaces; a tab is never indentation;
//!   2. the key/value separator is the first `:` followed by a space or
//!      end-of-line — a `:` inside a value is not one;
//!   3. a value's extent runs to the last non-space before an unquoted
//!      ` #` or end of line; a quoted value is delimited by its quotes;
//!   4. a single-line list entry is `- <scalar>` at the child indentation;
//!   5. `[id=NAME]` scans that entry's **own** lines only;
//!   6. not settable: a flow collection, a block scalar, a multi-line
//!      value; ambiguous: the path matches more than one line; no such
//!      key: it matches none;
//!   7. the scanner reads bytes and edits a copy; the target is never
//!      edited in place.
//!
//! Anchors are never searched heuristically and never approximated: a
//! near-miss here is a corrupted price table (spec §4.11 step 3).

use std::ops::Range;

/// A resolved anchor: the line's index, the byte extent of the value
/// **within that line** (the line taken without its terminator), whether
/// the line is currently commented out, and the value's quote style (the
/// codec reproduces the file's own style — the wizard changes a value,
/// never a style).
#[derive(Debug, Clone, PartialEq)]
pub struct Anchor {
    pub line: usize,
    pub range: Range<usize>,
    /// `false` when the key sits behind a leading `#` (the template ships
    /// `server.auth_token_env` and `plan_policy.overflow_monthly_cap_usd`
    /// commented out). `set-enabled` edits move this.
    pub enabled: bool,
    /// The comment marker's extent within the line (`# ` including its one
    /// following space when present) — what a `set-enabled` edit removes.
    pub marker: Option<Range<usize>>,
    pub quote: Option<char>,
    /// The current value as **displayed** (quotes stripped): the default a
    /// question shows is the file's own value, never a constant in code.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnchorError {
    NoSuchKey,
    /// The path matched more than one line (e.g. a duplicated key).
    Ambiguous(usize),
    /// The key is there but its value is not a single-line scalar in the
    /// accepted subset (flow collection, block scalar, block/empty value,
    /// multi-line value) — or a tab was used as indentation.
    NotSettable(&'static str),
}

impl AnchorError {
    pub fn reason(&self) -> &'static str {
        match self {
            AnchorError::NoSuchKey => "no line matches this key path",
            AnchorError::Ambiguous(_) => "more than one line matches this key path",
            AnchorError::NotSettable(why) => why,
        }
    }
}

// ---------------------------------------------------------------------------
// Path parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Sel {
    Index(usize),
    By { field: String, value: String },
}

#[derive(Debug, Clone)]
struct Seg {
    key: String,
    sel: Option<Sel>,
}

fn parse_path(path: &str) -> Result<Vec<Seg>, String> {
    let mut out = Vec::new();
    for part in path.split('.') {
        if part.is_empty() {
            return Err(format!("malformed key path `{path}`"));
        }
        let (key, sel) = match part.find('[') {
            None => (part.to_string(), None),
            Some(open) => {
                if !part.ends_with(']') {
                    return Err(format!("malformed key path `{path}`"));
                }
                let inner = &part[open + 1..part.len() - 1];
                let sel = if let Some(rest) = inner.strip_prefix("id=") {
                    Sel::By {
                        field: "id".to_string(),
                        value: rest.to_string(),
                    }
                } else if let Some(rest) = inner.strip_prefix("name=") {
                    Sel::By {
                        field: "name".to_string(),
                        value: rest.to_string(),
                    }
                } else if let Ok(i) = inner.parse::<usize>() {
                    Sel::Index(i)
                } else {
                    return Err(format!(
                        "malformed selector `[{inner}]` in `{path}` (expected \
                         [i], [id=NAME] or [name=NAME])"
                    ));
                };
                (part[..open].to_string(), Some(sel))
            }
        };
        out.push(Seg { key, sel });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Line classification
// ---------------------------------------------------------------------------

/// One significant line: its space indentation, its content after the
/// indent (with the terminator already removed), and the full original
/// line — anchors are addressed in **full-line** coordinates, because
/// `edit::apply` splices whole lines. `indent` is `None` for a
/// tab-indented line — "a tab is never indentation", so such a line never
/// matches a key and reads as a continuation (rule 1).
#[derive(Debug, Clone, Copy)]
struct Ln<'a> {
    indent: Option<usize>,
    content: &'a str,
    full: &'a str,
}

/// Byte shift from `content` back to `full` coordinates.
fn shift_of(ln: &Ln<'_>) -> usize {
    ln.full.len() - ln.content.len()
}

fn split_lines(text: &str) -> Vec<Ln<'_>> {
    text.lines()
        .map(|l| {
            let ind = l.len() - l.trim_start_matches(' ').len();
            let content = &l[ind..];
            let indent = if content.starts_with('\t') {
                None
            } else {
                Some(ind)
            };
            Ln {
                indent,
                content,
                full: l,
            }
        })
        .collect()
}

/// A parsed key line: where the key text starts/ends (within the **full**
/// line, indent included) and where the value begins.
#[derive(Debug, Clone)]
struct KeyLine {
    key_start: usize,
    key_end: usize,
    sep: usize,
    value_start: usize,
    /// Byte extent of the leading `# ` marker, when the key is commented.
    marker: Option<Range<usize>>,
}

/// The first `:` followed by a space or end-of-line, after the key text
/// (rule 2) — a `:` inside a value is not one, because it is not followed
/// by a space *before* any value exists… the separator is simply the first
/// `:`-then-space-or-EOL in the line, and key texts in this subset never
/// contain one.
fn parse_key(content: &str) -> Option<KeyLine> {
    // A commented-out key: `#` + spaces + the key (rule: only a single
    // leading `#` introduces one; `##` is a prose comment). The marker's
    // extent is the whole `#`-and-spaces prefix, so an enable edit that
    // removes it leaves the key at its own indentation.
    let (marker, after) = match content.strip_prefix('#') {
        Some(rest) => {
            let after = rest.trim_start_matches(' ');
            if after.is_empty() || after.starts_with('#') {
                return None;
            }
            (Some(0..content.len() - after.len()), after)
        }
        None => (None, content),
    };
    if after.is_empty() {
        return None;
    }
    let key_start = content.len() - after.len();
    let sep_rel = find_sep(after)?;
    let key_end = key_start + sep_rel;
    Some(KeyLine {
        key_start,
        key_end,
        sep: key_end,
        value_start: key_end + 1,
        marker,
    })
}

fn find_sep(s: &str) -> Option<usize> {
    let b = s.as_bytes();
    for (i, c) in b.iter().enumerate() {
        if *c == b':' && (i + 1 == b.len() || b[i + 1] == b' ') {
            return Some(i);
        }
    }
    None
}

fn key_text<'a>(content: &'a str, k: &KeyLine) -> &'a str {
    content[k.key_start..k.key_end].trim()
}

// ---------------------------------------------------------------------------
// Block walking
// ---------------------------------------------------------------------------

/// A block: the half-open line range a parent key owns (the contiguous
/// run of lines with a deeper indentation, rule 1), the indentation its
/// own keys sit at, and — for a sequence-entry block — the dash line that
/// may itself carry the entry's first key (`- name: deepseek`, rule 4/5).
#[derive(Debug, Clone)]
struct Block {
    start: usize,
    end: usize,
    indent: usize,
    dash_line: Option<usize>,
}

/// A line that is nothing but a comment (possibly indented). The example
/// carries continuation comments at *shallower* indent than the keys they
/// annotate (see `server.auth_token_env`'s trailing notes), so a comment
/// line never terminates a block walk — only a non-comment line at or
/// above the parent's indent does.
fn is_comment(ln: &Ln<'_>) -> bool {
    ln.content.starts_with('#')
}

fn children_block(lines: &[Ln], parent: usize) -> Option<Block> {
    let p_ind = lines[parent].indent?;
    let mut end = parent + 1;
    while end < lines.len() {
        let l = &lines[end];
        if l.content.trim().is_empty() || is_comment(l) {
            end += 1;
            continue;
        }
        match l.indent {
            Some(i) if i > p_ind => end += 1,
            _ => break,
        }
    }
    if end == parent + 1 {
        return None;
    }
    let indent = lines[parent + 1..end]
        .iter()
        .find(|l| !l.content.trim().is_empty() && !is_comment(l))
        .and_then(|l| l.indent)?;
    Some(Block {
        start: parent + 1,
        end,
        indent,
        dash_line: None,
    })
}

/// Key matches inside a block: lines at exactly the block's key indent
/// (commented or not — both are candidates, and two of them is an
/// ambiguity, not a guess), plus the dash line's own key for an
/// entry block (`- name: x`, rule 5: the entry's own lines only).
fn find_key(lines: &[Ln], block: &Block, key: &str) -> Result<usize, AnchorError> {
    let mut hits: Vec<usize> = Vec::new();
    if let Some(d) = block.dash_line {
        if let Some(k) = parse_dash_key(lines[d].content) {
            if key_text(lines[d].content, &k) == key {
                hits.push(d);
            }
        }
    }
    for (i, l) in lines.iter().enumerate().take(block.end).skip(block.start) {
        if l.indent != Some(block.indent) || l.content.trim().is_empty() {
            continue;
        }
        if let Some(k) = parse_key(l.content) {
            if key_text(l.content, &k) == key {
                hits.push(i);
            }
        }
    }
    match hits.len() {
        0 => Err(AnchorError::NoSuchKey),
        1 => Ok(hits[0]),
        n => Err(AnchorError::Ambiguous(n)),
    }
}

/// The key a dash line carries directly (`- name: x` → the `name` key), as
/// offsets within that line.
fn parse_dash_key(content: &str) -> Option<KeyLine> {
    let rest = content.trim_start_matches(' ');
    if !rest.starts_with("- ") && rest != "-" {
        return None;
    }
    let after = if rest == "-" { "" } else { &rest[2..] };
    let offset = content.len() - after.len();
    let k = parse_key(after)?;
    Some(KeyLine {
        key_start: offset + k.key_start,
        key_end: offset + k.key_end,
        sep: offset + k.sep,
        value_start: offset + k.value_start,
        marker: k.marker.map(|r| offset + r.start..offset + r.end),
    })
}

fn is_dash(content: &str) -> bool {
    let t = content.trim_start_matches(' ');
    t == "-" || t.starts_with("- ")
}

/// Enumerate the entries of the sequence a block holds: dash lines at the
/// block's child indent, each with its own span (rule 5).
fn seq_entries(lines: &[Ln], block: &Block) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = block.start;
    while i < block.end {
        let l = &lines[i];
        if l.indent == Some(block.indent) && is_dash(l.content) {
            let mut end = i + 1;
            while end < block.end {
                let m = &lines[end];
                if m.content.trim().is_empty() || is_comment(m) {
                    end += 1;
                    continue;
                }
                match m.indent {
                    Some(ind) if ind > block.indent => end += 1,
                    _ => break,
                }
            }
            out.push((i, end));
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

fn entry_block(lines: &[Ln], dash: usize, end: usize) -> Block {
    let dash_ind = lines[dash].indent.unwrap_or(0);
    let indent = lines[dash + 1..end]
        .iter()
        .find(|l| !l.content.trim().is_empty())
        .and_then(|l| l.indent)
        .unwrap_or(dash_ind + 2);
    Block {
        start: dash + 1,
        end,
        indent,
        dash_line: Some(dash),
    }
}

// ---------------------------------------------------------------------------
// Value extents (rule 3 / rule 6)
// ---------------------------------------------------------------------------

/// Compute the value extent for a key line. `content` is the line's
/// content (indent stripped); the returned anchor is in **full-line**
/// coordinates. Refuses everything the subset does not delimit exactly.
fn value_anchor(
    content: &str,
    k: &KeyLine,
    lines: &[Ln],
    line: usize,
) -> Result<Anchor, AnchorError> {
    let vs = k.value_start;
    let rest = &content[vs..];
    let trimmed = rest.trim_start_matches(' ');
    let vstart = vs + (rest.len() - trimmed.len());
    let not_settable = |why: &'static str| Err(AnchorError::NotSettable(why));

    if vstart >= content.len() {
        return not_settable("the value is not on this line (a block or empty value)");
    }
    let first = content.as_bytes()[vstart];
    let (range, quote) = match first {
        b'{' | b'[' => return not_settable("a flow collection is not a single-line scalar"),
        b'|' | b'>' => return not_settable("a block scalar is not a single-line scalar"),
        b'"' | b'\'' => {
            let q = first as char;
            let close = content[vstart + 1..]
                .find(q)
                .ok_or(AnchorError::NotSettable("an unterminated quoted value"))?;
            let end = vstart + 1 + close + 1;
            (vstart..end, Some(q))
        }
        _ => {
            // Plain scalar: to the last non-space before an unquoted ` #`
            // or the end of line (rule 3).
            let bytes = content.as_bytes();
            let mut end = content.len();
            let mut i = vstart;
            while i < bytes.len() {
                if bytes[i] == b' ' && bytes.get(i + 1) == Some(&b'#') {
                    end = i;
                    break;
                }
                i += 1;
            }
            while end > vstart && (bytes[end - 1] == b' ' || bytes[end - 1] == b'\t') {
                end -= 1;
            }
            (vstart..end, None)
        }
    };
    // A plain value continued on a deeper line that is neither a key nor a
    // list entry is a multi-line value — refused rather than half-edited.
    // Comment lines (including the deeper-indented continuation notes the
    // example carries under its commented keys) are not continuations of
    // the value: a ` #` note's own prose may sit on later lines.
    if quote.is_none() {
        if let Some(next) = lines[line + 1..]
            .iter()
            .find(|l| !l.content.trim().is_empty() && !is_comment(l))
        {
            let deeper =
                next.indent.unwrap_or(usize::MAX) > lines[line].indent.unwrap_or(usize::MAX);
            let looks_structural = parse_key(next.content).is_some() || is_dash(next.content);
            if deeper && !looks_structural {
                return not_settable("a multi-line value is not a single-line scalar");
            }
        }
    }
    let raw = &content[range.clone()];
    let value = match quote {
        Some(q) => raw.trim_matches(q).to_string(),
        None => raw.to_string(),
    };
    let mut a = Anchor {
        line,
        range,
        enabled: k.marker.is_none(),
        marker: k.marker.clone(),
        quote,
        value,
    };
    // Ranges above are relative to `content` (de-indented); anchors are
    // returned in **full-line** coordinates because `edit::apply` splices
    // whole lines.
    let shift = shift_of(&lines[line]);
    a.range.start += shift;
    a.range.end += shift;
    if let Some(m) = &mut a.marker {
        m.start += shift;
        m.end += shift;
    }
    Ok(a)
}

// ---------------------------------------------------------------------------
// The public resolve
// ---------------------------------------------------------------------------

/// Descend into a parent key's block. A parent whose value sits inline on
/// its own line — a flow collection (`server: { addr: … }`) or a block
/// scalar (`server: |`) — is rule 6's *not settable*, never a misleading
/// "no such key": the children the path wants live inside a value this
/// subset never edits.
fn descend(lines: &[Ln], parent: usize) -> Result<Block, AnchorError> {
    if let Some(k) = parse_key(lines[parent].content) {
        let rest = lines[parent].content[k.value_start..].trim_start_matches(' ');
        if let Some(first) = rest.as_bytes().first() {
            match first {
                b'{' | b'[' => {
                    return Err(AnchorError::NotSettable(
                        "a flow collection is not a single-line scalar",
                    ))
                }
                b'|' | b'>' => {
                    return Err(AnchorError::NotSettable(
                        "a block scalar is not a single-line scalar",
                    ))
                }
                _ => {}
            }
        }
    }
    children_block(lines, parent).ok_or(AnchorError::NoSuchKey)
}

/// Resolve a key path against the file's bytes to a unique anchor.
pub fn resolve(text: &str, path: &str) -> Result<Anchor, String> {
    match resolve_typed(text, path) {
        Ok(a) => Ok(a),
        Err(e) => Err(format!(
            "{path}: {} ({})",
            e.reason(),
            match e {
                AnchorError::Ambiguous(n) => format!("{n} lines matched"),
                _ => "line locator, spec §4.11 step 3".to_string(),
            }
        )),
    }
}

/// Same walk, typed — for the run's refusal ladder (which needs to know
/// *which* refusal it is: a requested change on a `NotSettable` key and an
/// unresolvable anchor are the same exit code, but different messages).
pub fn resolve_typed(text: &str, path: &str) -> Result<Anchor, AnchorError> {
    let segs = parse_path(path).map_err(|_| AnchorError::NoSuchKey)?;
    let lines = split_lines(text);
    let root = Block {
        start: 0,
        end: lines.len(),
        indent: 0,
        dash_line: None,
    };
    let mut block = root;
    let mut last_key_line = None;

    for (i, seg) in segs.iter().enumerate() {
        let is_last = i == segs.len() - 1;
        let ln = find_key(&lines, &block, &seg.key)?;
        match &seg.sel {
            None => {
                if is_last {
                    last_key_line = Some(ln);
                    break;
                }
                block = descend(&lines, ln)?;
            }
            Some(sel) => {
                let seq_block = descend(&lines, ln)?;
                let entries = seq_entries(&lines, &seq_block);
                let pick = match sel {
                    Sel::Index(n) => entries.get(*n).copied(),
                    Sel::By { field, value } => {
                        let mut hits = Vec::new();
                        for (d, e) in &entries {
                            let eb = entry_block(&lines, *d, *e);
                            if let Ok(f) = find_key(&lines, &eb, field) {
                                let k = parse_key(lines[f].content)
                                    .ok_or(AnchorError::NotSettable("an unparseable key line"))?;
                                if let Ok(a) = value_anchor(lines[f].content, &k, &lines, f) {
                                    if &a.value == value {
                                        hits.push((*d, *e));
                                    }
                                }
                            }
                        }
                        match hits.len() {
                            0 => None,
                            1 => Some(hits[0]),
                            _ => return Err(AnchorError::Ambiguous(hits.len())),
                        }
                    }
                };
                let (dash, end) = pick.ok_or(AnchorError::NoSuchKey)?;
                if is_last {
                    // `a.b[i]`: the entry itself must be a single-line
                    // scalar (`- <scalar>`), and the anchor is its extent.
                    let ln = &lines[dash];
                    let content = ln.content;
                    let t = content.trim_start_matches(' ');
                    if t == "-" {
                        return Err(AnchorError::NotSettable("an empty list entry"));
                    }
                    let after = &t[2..];
                    let off = shift_of(ln) + (content.len() - after.len());
                    if after.starts_with('{') || after.starts_with('[') {
                        return Err(AnchorError::NotSettable(
                            "a flow collection is not a single-line scalar",
                        ));
                    }
                    // ADR-052 §2.1: the documented form is `- <scalar>`.
                    // An entry that is a **block mapping** (its first key
                    // riding its own dash line, `- family: x`, or a key on
                    // a following line) is structure, never a value: the
                    // terminal selector is refused rather than answered
                    // with the dash line's own text, whose extent a
                    // `set-value` edit would splice as if it were a value.
                    if parse_key(after).is_some() {
                        return Err(AnchorError::NotSettable(
                            "a mapping list entry is not a single-line scalar",
                        ));
                    }
                    let mut e = ln.full.len();
                    let b = ln.full.as_bytes();
                    let mut i = off;
                    while i < b.len() {
                        if b[i] == b' ' && b.get(i + 1) == Some(&b'#') {
                            e = i;
                            break;
                        }
                        i += 1;
                    }
                    while e > off && (b[e - 1] == b' ' || b[e - 1] == b'\t') {
                        e -= 1;
                    }
                    return Ok(Anchor {
                        line: dash,
                        range: off..e,
                        enabled: true,
                        marker: None,
                        quote: None,
                        value: ln.full[off..e].to_string(),
                    });
                }
                block = entry_block(&lines, dash, end);
            }
        }
    }

    let ln = last_key_line.ok_or(AnchorError::NoSuchKey)?;
    let content = lines[ln].content;
    let k = parse_key(content).ok_or(AnchorError::NotSettable(
        "the line does not carry a parseable key",
    ))?;
    value_anchor(content, &k, &lines, ln)
}

// ---------------------------------------------------------------------------
// Enumeration (the section table's dynamic rows)
// ---------------------------------------------------------------------------

/// The `name:` (or `id:`) value of every entry of a root-level sequence —
/// how the `providers` / `plugins` sections enumerate what the file carries.
pub fn entry_names(text: &str, root_key: &str, field: &str) -> Vec<String> {
    let lines = split_lines(text);
    let root = Block {
        start: 0,
        end: lines.len(),
        indent: 0,
        dash_line: None,
    };
    let Ok(ln) = find_key(&lines, &root, root_key) else {
        return Vec::new();
    };
    let Some(sb) = children_block(&lines, ln) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (d, e) in seq_entries(&lines, &sb) {
        let eb = entry_block(&lines, d, e);
        if let Ok(f) = find_key(&lines, &eb, field) {
            if let Some(k) = parse_key(lines[f].content) {
                if let Ok(a) = value_anchor(lines[f].content, &k, &lines, f) {
                    out.push(a.value);
                }
            }
        }
    }
    out
}

/// The keys a root-level block mapping carries (`aliases`), in file order —
/// the display-only membership lists, shown one line per entry.
pub fn block_keys(text: &str, root_key: &str) -> Vec<String> {
    let lines = split_lines(text);
    let root = Block {
        start: 0,
        end: lines.len(),
        indent: 0,
        dash_line: None,
    };
    let Ok(ln) = find_key(&lines, &root, root_key) else {
        return Vec::new();
    };
    let Some(b) = children_block(&lines, ln) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for l in &lines[b.start..b.end] {
        if l.indent == Some(b.indent) {
            if let Some(k) = parse_key(l.content) {
                let key = key_text(l.content, &k);
                if !key.starts_with('#') {
                    out.push(key.to_string());
                }
            }
        }
    }
    out
}

/// The entry count of a root-level sequence (`fallback`).
pub fn seq_len(text: &str, root_key: &str) -> usize {
    let lines = split_lines(text);
    let root = Block {
        start: 0,
        end: lines.len(),
        indent: 0,
        dash_line: None,
    };
    let Ok(ln) = find_key(&lines, &root, root_key) else {
        return 0;
    };
    children_block(&lines, ln)
        .map(|sb| seq_entries(&lines, &sb).len())
        .unwrap_or(0)
}

/// The line span a **top-level** key owns: the key's own line and the last
/// line its block holds (0-based, **inclusive**) — the shape step's unit
/// (ADR-038; spec §4.11). The rules are this module's own: comment lines
/// never terminate a block (rule of `children_block`, so an annotation
/// travels with what it annotates), **trailing blank lines are not part
/// of the span** (they separate the block from the next key, and they
/// stay where they are), a trailing **comment run** is the block's own
/// only when it is attached to it — a blank line between the block and
/// the run makes the run the next section's banner, and it stays too —
/// and a key whose value sits on its own line (`providers: []`, a flow
/// collection) spans that line alone.
///
/// Refusals are the locator's (`NoSuchKey` when no line carries the key,
/// `Ambiguous` when more than one does — a commented copy beside an
/// enabled one is two lines, and the shape step refuses rather than
/// guessing which block to move).
pub fn block_span(text: &str, key: &str) -> Result<(usize, usize), AnchorError> {
    let lines = split_lines(text);
    let root = Block {
        start: 0,
        end: lines.len(),
        indent: 0,
        dash_line: None,
    };
    let header = find_key(&lines, &root, key)?;
    let last = match children_block(&lines, header) {
        None => header,
        Some(b) => {
            let mut end = b.end;
            loop {
                // The last owned line is the last non-blank one.
                let Some(i) = lines[b.start..end]
                    .iter()
                    .rposition(|l| !l.content.trim().is_empty())
                    .map(|i| b.start + i)
                else {
                    break header;
                };
                if !is_comment(&lines[i]) {
                    break i;
                }
                // The trailing run of comment lines ending at `i`:
                // attached to the block (no blank line above it) it is
                // the block's own annotation and travels; detached, it
                // is the next section's banner and stays.
                let run_start = (b.start..i)
                    .rev()
                    .take_while(|&j| is_comment(&lines[j]))
                    .last()
                    .unwrap_or(i);
                if run_start == b.start || !lines[run_start - 1].content.trim().is_empty() {
                    break i;
                }
                end = run_start;
            }
        }
    };
    Ok((header, last))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = r#"# header comment
server:
  addr: "127.0.0.1:8790"        # local-first
  # auth_token_env: VADIS_TOKEN # optional (spec §4.7)
  request_timeout: 10m          # budget
session:
  key_sources: ["prompt_cache_key", "header:session-id"]
  ttl: 12h
cache:
  sticky: true
  breakeven:
    safety_factor: 1.2
providers:
  - name: deepseek
    region: intl
    api_key_env: DEEPSEEK_API_KEY
    models:
      - id: deepseek-v4-pro
        context: 1m
      - id: deepseek-flash
        context: 1m
  - name: zai
    api_key_env: ZAI_API_KEY

plugins:
  - id: cache-guard
    kind: builtin/cache_guard
    config: { strict_prefix: true }
  - id: tool-output-rules
    kind: builtin/transform_rules
    config:
      rules_file: ./rules/tool_output.toml
    disabled: true
aliases:
  coding-fast: deepseek/deepseek-flash
  glm-plan: zai/glm-5.3
fallback:
  - deepseek/deepseek-v4-pro
  - zai/glm-5.3
plan_policy:
  family: glm-5.3
  # overflow_monthly_cap_usd: 20.0   # optional
  cooldown: 15m
"#;

    #[test]
    fn plain_and_quoted_values() {
        let a = resolve_typed(DOC, "server.addr").unwrap();
        assert_eq!(a.value, "127.0.0.1:8790");
        assert_eq!(a.quote, Some('"'));
        assert_eq!(
            &DOC.lines().nth(a.line).unwrap()[a.range.clone()],
            "\"127.0.0.1:8790\""
        );

        let t = resolve_typed(DOC, "session.ttl").unwrap();
        assert_eq!(t.value, "12h");
        assert_eq!(t.quote, None);
    }

    #[test]
    fn trailing_comment_is_not_part_of_the_value() {
        let a = resolve_typed(DOC, "server.request_timeout").unwrap();
        assert_eq!(a.value, "10m");
    }

    #[test]
    fn commented_key_resolves_disabled_with_its_value() {
        let a = resolve_typed(DOC, "server.auth_token_env").unwrap();
        assert!(!a.enabled);
        assert_eq!(a.value, "VADIS_TOKEN");
        assert!(a.marker.is_some());
        let cap = resolve_typed(DOC, "plan_policy.overflow_monthly_cap_usd").unwrap();
        assert!(!cap.enabled);
        assert_eq!(cap.value, "20.0");
    }

    #[test]
    fn nested_blocks_walk_by_indentation() {
        let a = resolve_typed(DOC, "cache.breakeven.safety_factor").unwrap();
        assert_eq!(a.value, "1.2");
    }

    #[test]
    fn flow_collections_are_not_settable() {
        assert_eq!(
            resolve_typed(DOC, "session.key_sources"),
            Err(AnchorError::NotSettable(
                "a flow collection is not a single-line scalar"
            ))
        );
        assert_eq!(
            resolve_typed(DOC, "plugins[id=cache-guard].config"),
            Err(AnchorError::NotSettable(
                "a flow collection is not a single-line scalar"
            ))
        );
    }

    #[test]
    fn list_entry_selection_by_name_and_id() {
        let a = resolve_typed(DOC, "providers[name=deepseek].api_key_env").unwrap();
        assert_eq!(a.value, "DEEPSEEK_API_KEY");
        let b = resolve_typed(DOC, "plugins[id=tool-output-rules].config.rules_file").unwrap();
        assert_eq!(b.value, "./rules/tool_output.toml");
        // rule 5: a key of a *different* entry is never matched by text alone
        assert_eq!(
            resolve_typed(DOC, "providers[name=nope].api_key_env"),
            Err(AnchorError::NoSuchKey)
        );
    }

    #[test]
    fn dash_line_key_is_the_entrys_own() {
        let a = resolve_typed(DOC, "providers[name=zai].name").unwrap();
        assert_eq!(a.value, "zai");
    }

    #[test]
    fn indexed_sequence_entry_scalar() {
        let a = resolve_typed(DOC, "fallback[0]").unwrap();
        assert_eq!(a.value, "deepseek/deepseek-v4-pro");
        assert_eq!(
            resolve_typed(DOC, "fallback[2]"),
            Err(AnchorError::NoSuchKey)
        );
    }

    /// ADR-052 §2.1: a **terminal** selector whose entry is a block
    /// mapping is *not settable* — the entry as a whole is structure,
    /// never a single-line scalar, and the dash line's own text is not a
    /// value the wizard may splice. The scalar-entry form is unchanged.
    #[test]
    fn terminal_selector_on_a_mapping_entry_is_not_settable() {
        for path in [
            "providers[0]",
            "providers[name=deepseek]",
            "plugins[id=cache-guard]",
        ] {
            assert_eq!(
                resolve_typed(DOC, path),
                Err(AnchorError::NotSettable(
                    "a mapping list entry is not a single-line scalar"
                )),
                "{path} must be refused, never answered with the dash line's text"
            );
        }
        // A `- <scalar>` entry still resolves to the scalar itself.
        assert_eq!(
            resolve_typed(DOC, "fallback[0]").unwrap().value,
            "deepseek/deepseek-v4-pro"
        );
    }

    #[test]
    fn block_mapping_keys_and_duplication_refusals() {
        assert_eq!(
            resolve_typed(DOC, "aliases.coding-fast").unwrap().value,
            "deepseek/deepseek-flash"
        );
        let dup = DOC.replace("  cooldown: 15m", "  cooldown: 15m\n  cooldown: 16m");
        assert!(matches!(
            resolve_typed(&dup, "plan_policy.cooldown"),
            Err(AnchorError::Ambiguous(2))
        ));
        assert_eq!(
            resolve_typed(DOC, "server.missing"),
            Err(AnchorError::NoSuchKey)
        );
    }

    #[test]
    fn ambiguous_across_commented_and_active_is_refused() {
        let dup = DOC.replace("  cooldown: 15m", "  cooldown: 15m\n  # cooldown: 15m");
        assert!(matches!(
            resolve_typed(&dup, "plan_policy.cooldown"),
            Err(AnchorError::Ambiguous(2))
        ));
    }

    #[test]
    fn flow_mapping_rewrite_is_not_settable() {
        // CONF-68(c)'s shape: the server block rewritten as a flow mapping.
        let mangled =
            "server: { addr: \"127.0.0.1:8790\", request_timeout: 10m }\nsession:\n  ttl: 12h\n";
        assert_eq!(
            resolve_typed(mangled, "server.addr"),
            Err(AnchorError::NotSettable(
                "a flow collection is not a single-line scalar"
            ))
        );
    }

    #[test]
    fn multiline_plain_value_is_not_settable() {
        let mangled = "session:\n  ttl: 12h\n    continued oops\n";
        assert!(matches!(
            resolve_typed(mangled, "session.ttl"),
            Err(AnchorError::NotSettable(_))
        ));
    }

    #[test]
    fn block_scalar_is_not_settable() {
        let mangled = "session:\n  ttl: |\n    12h\n";
        assert!(matches!(
            resolve_typed(mangled, "session.ttl"),
            Err(AnchorError::NotSettable(_))
        ));
    }

    #[test]
    fn tab_indentation_never_matches_a_key() {
        let mangled = "session:\n\tttl: 12h\n";
        assert_eq!(
            resolve_typed(mangled, "session.ttl"),
            Err(AnchorError::NoSuchKey)
        );
    }

    #[test]
    fn enumeration_helpers() {
        assert_eq!(
            entry_names(DOC, "providers", "name"),
            vec!["deepseek".to_string(), "zai".to_string()]
        );
        assert_eq!(
            entry_names(DOC, "plugins", "id"),
            vec!["cache-guard".to_string(), "tool-output-rules".to_string()]
        );
        assert_eq!(seq_len(DOC, "fallback"), 2);
        assert_eq!(block_keys(DOC, "aliases"), vec!["coding-fast", "glm-plan"]);
    }

    /// The shape step's unit (ADR-038; spec §4.11): the span is the header
    /// line through the last line the block owns — comment lines do not
    /// terminate it, trailing blank lines are not part of it, and a key
    /// whose value sits on its own line spans that line alone.
    #[test]
    fn block_spans_are_the_header_through_the_last_owned_line() {
        let (h, l) = block_span(DOC, "providers").unwrap();
        let lines: Vec<&str> = DOC.lines().collect();
        assert_eq!(lines[h], "providers:");
        assert_eq!(lines[l].trim(), "api_key_env: ZAI_API_KEY");
        // The blank line before `plugins:` stays outside the span.
        assert_eq!(lines[l + 1].trim(), "");
        assert_eq!(lines[l + 2], "plugins:");

        // A comment line at the block's own indent never terminates it: it
        // is part of what gets moved.
        let commented = DOC.replace("plugins:\n", "# review before shipping\nplugins:\n");
        let (h2, l2) = block_span(&commented, "providers").unwrap();
        assert_eq!((h2, l2), (h, l), "the span ends before the new comment");

        // A single-line value spans that line alone.
        let flow = "trace:\n  dir: ./traces\nproviders: []\naliases:\n  a: b\n";
        assert_eq!(block_span(flow, "providers"), Ok((2, 2)));

        // Refusals: no such line, and two lines claiming the same key.
        assert_eq!(
            block_span("trace:\n  dir: ./traces\n", "providers"),
            Err(AnchorError::NoSuchKey)
        );
        let two = format!("{DOC}# providers:\n");
        assert_eq!(
            block_span(&two, "providers"),
            Err(AnchorError::Ambiguous(2))
        );
    }

    /// A block's **own comment** travels with it: the example annotates
    /// prices and endpoints, so a span that stopped at a comment would
    /// leave those lines behind (and the price table would lose its
    /// citations).
    #[test]
    fn block_span_keeps_the_blocks_own_comments() {
        let doc = "server:\n  addr: \"127.0.0.1:8790\"\nproviders:\n  # source: https://example.com @2026-09-26\n  - name: p\n    api_key_env: K\n# a section banner\nplugins: []\n";
        let (h, l) = block_span(doc, "providers").unwrap();
        let lines: Vec<&str> = doc.lines().collect();
        assert_eq!(lines[h], "providers:");
        assert_eq!(lines[l], "# a section banner");
    }

    #[test]
    fn every_row_of_the_real_example_resolves() {
        // The table↔example check (DESIGN §12.14): every static key path
        // the section table carries must resolve in the shipped example
        // **of its target file** (spec §4.11's target-file column): the
        // example is a pair since §4.14's split, so the root's keys are
        // read from `config.example.yaml` and the roster's from
        // `providers.example.yaml`.
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let example = std::fs::read_to_string(dir.join("config.example.yaml"))
            .expect("shipped example is readable");
        let roster = std::fs::read_to_string(dir.join("providers.example.yaml"))
            .expect("shipped roster example is readable");
        for path in [
            "server.addr",
            "server.upstream_attempt_timeout",
            "server.request_timeout",
            "server.auth_token_env",
            "session.ttl",
            "cache.sticky",
            "cache.breakeven.enabled",
            "cache.breakeven.min_remaining_turns",
            "cache.breakeven.safety_factor",
            "trace.dir",
            "trace.rollover",
        ] {
            assert!(
                resolve(&example, path).is_ok(),
                "the section table's `{path}` must resolve in config.example.yaml"
            );
        }
        // ADR-052 §2.3(b): the shipped root writes the **list** spelling, so
        // the property is read over the list — for **each declared index** of
        // the shipped root and each of the eight policy keys,
        // `plan_policies[i].<key>` must resolve. The index list comes from the
        // file itself (`entry_names`-style enumeration), never from a constant:
        // a third family added by hand moves this test with it, and the eight
        // keys are the section table's own set (a commented-out cap included,
        // which is what keeps the per-family `set-enabled` edit reachable).
        let declared = entry_names(&example, "plan_policies", "family");
        assert!(
            !declared.is_empty(),
            "the shipped root declares a plan family (ADR-052's flip)"
        );
        for i in 0..declared.len() {
            for key in [
                "family",
                "primary",
                "overflow",
                "on_primary_exhausted",
                "recover",
                "cooldown",
                "overflow_selection",
                "overflow_monthly_cap_usd",
            ] {
                let path = format!("plan_policies[{i}].{key}");
                assert!(
                    resolve(&example, &path).is_ok(),
                    "the section table's `{path}` must resolve in config.example.yaml"
                );
            }
        }
        for name in entry_names(&roster, "providers", "name") {
            let p = format!("providers[name={name}].api_key_env");
            assert!(
                resolve(&roster, &p).is_ok(),
                "{p} must resolve in providers.example.yaml"
            );
        }
        for id in entry_names(&example, "plugins", "id") {
            let p = format!("plugins[id={id}].disabled");
            // `disabled` exists on judge-experiment only; a missing anchor
            // is the *warning* shape, so resolve_typed's Err is fine there
            // — but the entry itself must be found:
            let q = format!("plugins[id={id}].kind");
            assert!(resolve(&example, &q).is_ok(), "{q} must resolve");
            let _ = p;
        }
    }
}
