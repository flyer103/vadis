//! Byte-faithful primitive `RawBody` (DESIGN §12.3.1, ADR-007 "span-faithful
//! forwarding", AGENTS hard constraints 1/2).
//!
//! Two byte-level mutations are permitted, both span-scoped (spec §2, DESIGN
//! §12.3.1): **(a)** removing router-owned top-level fields, and **(b)**
//! replacing the value of a top-level string member (the outbound `model`,
//! §12.10.7). Every other byte is preserved verbatim. The implementation is a
//! **single-pass span scanner** (tracking strings/escapes/bracket depth) that
//! locates the byte ranges to edit and splices/excises those ranges — a
//! **parse → reserialize round trip is forbidden** (that is the most common
//! way to break the byte boundary). `serde_json` is used only as a
//! **validator** for raw value fragments (true/false/null/number), never to
//! produce any outbound bytes.
//!
//! Container deviation: the DESIGN sketch draws `RawBody(Bytes)`, but the
//! `bytes` crate is not in `router-core`'s dependency whitelist (§12.1: only
//! serde/serde_json), so `Vec<u8>` is used instead. The zero-copy outbound
//! conversion (`Bytes::from(vec)`) happens in the proxy layer; semantics
//! are unaffected.

use std::borrow::Cow;

/// Whitelist of router-owned top-level keys (spec §2 / DESIGN §12.3.1).
///
/// This is the **only** deletion list: `router_meta` echo + routing hints.
/// New router-owned keys must be added here — a second scattered copy of the
/// list at call sites is not allowed. Note: legitimate unknown fields in
/// client requests (CONF-11) are **not** in this list and are never removed.
pub const ROUTER_OWNED_TOP_LEVEL_KEYS: &[&str] = &["router_meta"];

/// Failure semantics of `remove_top_level_keys` (three distinguishable
/// failures so callers can handle them separately).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawEditError {
    /// Body is empty or whitespace-only (no scannable JSON).
    EmptyBody,
    /// Top level is not a JSON object (array/string/number/true/false/null are
    /// all rejected).
    NotTopLevelObject { first_byte: u8 },
    /// Invalid JSON structure: unclosed string/bracket, invalid escape,
    /// trailing content, empty value, etc.
    Malformed { offset: usize },
    /// `set_top_level_string`: no top-level member bears the key. Mutation
    /// (b) is a **replacement**, never an insertion — the passthrough path
    /// must not invent a member the client did not send (DESIGN §12.3.1).
    TopLevelKeyAbsent,
    /// `set_top_level_string`: the member's value is not a JSON string
    /// (number/bool/null/array/object are all rejected).
    TopLevelValueNotString { first_byte: u8 },
}

/// Raw client bytes: the sole authority. Does not implement
/// `DerefMut`/`AsMut` and exposes no mutable view, blocking "just a quick
/// tweak" at compile time (DESIGN §12.3.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawBody(Vec<u8>);

impl RawBody {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// The top-level members as (decoded key, value start, value end) in
    /// document order. Read-only span access for the prefix-block extractor
    /// (DESIGN §12.10.6: extraction is a span scan, never a
    /// parse→reserialize; the extractor and the deleter share the scanner).
    pub fn top_level_member_spans(&self) -> Result<Vec<(String, usize, usize)>, RawEditError> {
        let b = &self.0;
        let members = scan_top_level_members(b)?;
        members
            .into_iter()
            .map(|m| {
                let key = decode_json_string(b, m.key_start, m.key_end)?;
                Ok((key, m.val_start, m.val_end))
            })
            .collect()
    }

    /// Mutation (a) of the byte boundary: delete whitelisted top-level keys,
    /// preserve every other byte verbatim.
    ///
    /// - `keys` empty or all missed → identity (returns a byte-identical copy);
    /// - idempotent: two consecutive calls == one call;
    /// - pure function: depends only on (content, keys), never on clock/RNG/
    ///   turn number (AGENTS hard constraint 2).
    pub fn remove_top_level_keys(&self, keys: &[&str]) -> Result<RawBody, RawEditError> {
        let b = &self.0;
        let members = scan_top_level_members(b)?;

        // Members to delete are excised as whole "runs" of consecutive members:
        // a run swallows the separator comma between the run's tail and the
        // succeeding member. Commas inside the run disappear together with the
        // run's bytes, while the comma between the run's head and the previous
        // retained member is left to the latter — exactly where adjacent
        // deletions used to leave a dangling comma. A run consisting of
        // a single first member has no preceding comma to rely on, so it
        // swallows its trailing comma instead.
        let mut dels: Vec<(usize, usize)> = Vec::new();
        let mut idx = 0;
        while idx < members.len() {
            let m = &members[idx];
            let decoded_key = decode_json_string(b, m.key_start, m.key_end)?;
            if !keys.iter().any(|k| *k == decoded_key) {
                idx += 1;
                continue;
            }
            // Run head: locate the run tail (the last consecutive key hit).
            let mut run_last = idx;
            while run_last + 1 < members.len() {
                let next = &members[run_last + 1];
                let next_key = decode_json_string(b, next.key_start, next.key_end)?;
                if keys.iter().any(|k| *k == next_key) {
                    run_last += 1;
                } else {
                    break;
                }
            }
            let mut s = members[idx].key_start;
            let mut e = members[run_last].val_end;
            if idx == 0 && run_last + 1 < members.len() {
                // First run with a retained successor: swallow the trailing
                // comma (including whitespace between value and comma).
                let mut j = e;
                while j < b.len() && is_ws(b[j]) {
                    j += 1;
                }
                if j < b.len() && b[j] == b',' {
                    e = j + 1;
                }
            } else if idx > 0 {
                // Swallow the leading comma (including whitespace between the
                // run's first key and the comma). The trailing comma is left
                // for the succeeding retained member; commas inside the run are
                // deleted together with the run's bytes.
                let mut j = s;
                while j > 0 && is_ws(b[j - 1]) {
                    j -= 1;
                }
                if j > 0 && b[j - 1] == b',' {
                    s = j - 1;
                }
            }
            dels.push((s, e));
            idx = run_last + 1;
        }

        if dels.is_empty() {
            return Ok(self.clone());
        }
        dels.sort_unstable();
        let mut merged: Vec<(usize, usize)> = Vec::with_capacity(dels.len());
        for (s, e) in dels {
            match merged.last_mut() {
                Some((_, le)) if s <= *le => {
                    // Overlapping or adjacent (sharing a comma) → take the
                    // union.
                    *le = (*le).max(e);
                }
                _ => merged.push((s, e)),
            }
        }

        let mut out = Vec::with_capacity(b.len());
        let mut pos = 0usize;
        for (s, e) in &merged {
            out.extend_from_slice(&b[pos..*s]);
            pos = *e;
        }
        out.extend_from_slice(&b[pos..]);
        Ok(RawBody(out))
    }

    /// Mutation (b) of the byte boundary (DESIGN §12.3.1 / §12.10.7): replace
    /// the **value** of a top-level string member with `value`, byte-level,
    /// single pass, no parse→reserialize.
    ///
    /// Only the member's value span moves: its key bytes, its position, the
    /// separators, the whitespace around it and every byte outside the member
    /// are byte-for-byte the input; the new value is emitted as a JSON string
    /// (`"` + RFC 8259 escaping + `"`).
    ///
    /// - value equal to the member's current value → `Cow::Borrowed` (the
    ///   input bytes, unchanged);
    /// - otherwise `Cow::Owned`;
    /// - absent member → `Err(TopLevelKeyAbsent)` — never an insertion;
    /// - present member whose value is not a JSON string →
    ///   `Err(TopLevelValueNotString)`;
    /// - duplicate top-level keys → only the **last** occurrence is replaced
    ///   (RFC 8259 §4: "last wins" is the decodable semantics; the earlier
    ///   duplicates are left verbatim, like every other client byte);
    /// - pure function of (content, key, value) — no clock, no RNG, no turn
    ///   number (AGENTS hard constraint 2).
    pub fn set_top_level_string(
        &self,
        key: &str,
        value: &str,
    ) -> Result<Cow<'_, [u8]>, RawEditError> {
        let b = self.as_bytes();
        // The same shared scanner as remove_top_level_keys (DESIGN §12.3.1:
        // one scanner, two policies — never a second parser).
        let members = scan_top_level_members(b)?;

        // Duplicates: replace the LAST occurrence — the decodable value of
        // the member (RFC 8259 §4 "last wins", the same rule serde_json's
        // map applies). Earlier duplicates are client bytes and stay
        // verbatim.
        let mut target: Option<&MemberSpan> = None;
        for m in &members {
            if decode_json_string(b, m.key_start, m.key_end)? == key {
                target = Some(m);
            }
        }
        let m = target.ok_or(RawEditError::TopLevelKeyAbsent)?;

        if b[m.val_start] != b'"' {
            return Err(RawEditError::TopLevelValueNotString {
                first_byte: b[m.val_start],
            });
        }

        // Decoded equality → Cow::Borrowed: the input bytes are already the
        // answer, escapes and all (DESIGN §12.3.1).
        if decode_json_string(b, m.val_start, m.val_end)? == value {
            return Ok(Cow::Borrowed(b));
        }

        // Splice: everything before the value span, the new encoded string,
        // everything after. No other byte is visited.
        let mut out = Vec::with_capacity(b.len() + value.len());
        out.extend_from_slice(&b[..m.val_start]);
        append_json_string(&mut out, value);
        out.extend_from_slice(&b[m.val_end..]);
        Ok(Cow::Owned(out))
    }

    /// The transform-mode splicer (ADR-019 / DESIGN §12.12): one level
    /// deeper than `set_top_level_string` — replace the value spans
    /// addressed by `edits` (paths such as `messages[3].content`), splicing
    /// each node's new text re-encoded as a JSON string. Every byte outside
    /// the declared spans is the input's, byte for byte; a parse →
    /// reserialize round trip is forbidden here exactly as it is on the
    /// passthrough path. Same escaping rule, same "an unresolvable address
    /// is an error, never an invention" stance (§12.3.1).
    ///
    /// - `edits` empty → `Cow::Borrowed` (the input bytes, unchanged);
    /// - paths are resolved against **these** bytes first (all of them,
    ///   before any splice: the application is all-or-nothing), then the
    ///   edits are spliced in ascending span order into a fresh buffer,
    ///   copying the untouched spans from the input — so every offset is
    ///   computed against the original bytes and never shifts mid-apply;
    /// - a path that resolves to an absent node or a non-string value is
    ///   `Err(Malformed)` — the caller's fail-safe serves the payload
    ///   verbatim, never a partially edited body (spec §8);
    /// - two edits resolving to the same span are `Err(Malformed)`;
    /// - pure function of (bytes, edits) — no clock, no RNG, no turn number
    ///   (AGENTS hard constraint 2).
    pub fn apply_edits(
        &self,
        edits: &[crate::transform::PayloadEdit],
    ) -> Result<Cow<'_, [u8]>, RawEditError> {
        if edits.is_empty() {
            return Ok(Cow::Borrowed(self.as_bytes()));
        }
        let b = self.as_bytes();
        // Resolve every span first (fail before any splice: all-or-nothing).
        let mut spans: Vec<usize> = Vec::with_capacity(edits.len());
        for e in edits {
            spans.push(resolve_string_value_start(b, &e.path)?);
        }
        // Splice in ascending span order into a fresh buffer; `pos` is an
        // offset into the *input*, so replacements never shift it.
        let mut order: Vec<usize> = (0..edits.len()).collect();
        order.sort_by_key(|&i| spans[i]);
        let mut out = Vec::with_capacity(b.len());
        let mut pos = 0usize;
        for &i in &order {
            let s = spans[i];
            // The value is a JSON string starting at s; its closing quote is
            // resolved by the same escape-aware scanner.
            let val_end = scan_json_string(b, s)?;
            if s < pos {
                // Overlapping/duplicate spans: an unresolvable plan, never
                // an invention.
                return Err(RawEditError::Malformed { offset: s });
            }
            out.extend_from_slice(&b[pos..s]);
            append_json_string(&mut out, &edits[i].new_text);
            pos = val_end;
        }
        out.extend_from_slice(&b[pos..]);
        Ok(Cow::Owned(out))
    }
}

/// Walk `path` from the top level and return the byte offset of the addressed
/// node's value start (the opening quote for a string value). Key lookup
/// walks object members with the shared scanner (decoded-equality key
/// matching, last occurrence wins); index lookup walks array elements with
/// the bracket-tracking scan. Absent nodes and non-string targets are
/// `Err(Malformed)` — an unresolvable address is an error, never an
/// invention.
fn resolve_string_value_start(
    b: &[u8],
    path: &crate::transform::NodePath,
) -> Result<usize, RawEditError> {
    use crate::transform::PathSeg;

    let mut cursor: (usize, usize) = (0, b.len()); // the value span we are inside
    for seg in path.0.iter() {
        match seg {
            PathSeg::Key(key) => {
                // The cursor must be an object.
                let s = skip_ws(b, cursor.0);
                if s >= cursor.1 || b[s] != b'{' {
                    return Err(RawEditError::Malformed { offset: s });
                }
                let members = scan_object_members(b, s, cursor.1)?;
                let mut found: Option<(usize, usize)> = None;
                for m in &members {
                    if decode_json_string(b, m.key_start, m.key_end)? == *key {
                        found = Some((m.val_start, m.val_end));
                    }
                }
                let Some((vs, ve)) = found else {
                    return Err(RawEditError::Malformed { offset: s });
                };
                cursor = (vs, ve);
            }
            PathSeg::Index(n) => {
                let s = skip_ws(b, cursor.0);
                if s >= cursor.1 || b[s] != b'[' {
                    return Err(RawEditError::Malformed { offset: s });
                }
                let mut i = s + 1;
                let mut seen = 0u32;
                let mut found: Option<(usize, usize)> = None;
                loop {
                    let j = skip_ws(b, i);
                    if j >= cursor.1 || b[j] == b']' {
                        break;
                    }
                    let el_end = crate::prefix::scan_element_end(b, j, cursor.1);
                    if seen == *n {
                        found = Some((j, el_end));
                        break;
                    }
                    seen += 1;
                    i = el_end;
                    let k = skip_ws(b, i);
                    if k < cursor.1 && b[k] == b',' {
                        i = k + 1;
                    } else {
                        break;
                    }
                }
                let Some((vs, ve)) = found else {
                    return Err(RawEditError::Malformed { offset: s });
                };
                cursor = (vs, ve);
            }
        }
    }
    // The final node must be a JSON string value.
    let s = skip_ws(b, cursor.0);
    if s < cursor.1 && b[s] == b'"' {
        Ok(s)
    } else {
        Err(RawEditError::Malformed { offset: s })
    }
}

/// The members of the object whose opening brace is at `open`, bounded by
/// `limit`. Same span discipline as the top-level scanner, one level deeper.
fn scan_object_members(
    b: &[u8],
    open: usize,
    limit: usize,
) -> Result<Vec<MemberSpan>, RawEditError> {
    let mut members = Vec::new();
    let mut i = open + 1;
    loop {
        i = skip_ws(b, i);
        if i >= limit || b[i] == b'}' {
            break;
        }
        if !members.is_empty() {
            if b[i] == b',' {
                i = skip_ws(b, i + 1);
            } else if b[i] != b'}' {
                return Err(RawEditError::Malformed { offset: i });
            }
        }
        if i >= limit || b[i] == b'}' {
            break;
        }
        if b[i] != b'"' {
            return Err(RawEditError::Malformed { offset: i });
        }
        let key_start = i;
        let key_end = scan_json_string(b, i)?;
        let colon = skip_ws(b, key_end);
        if colon >= limit || b[colon] != b':' {
            return Err(RawEditError::Malformed { offset: colon });
        }
        let val_start = skip_ws(b, colon + 1);
        let val_end = crate::prefix::scan_element_end(b, val_start, limit);
        if val_end > limit {
            return Err(RawEditError::Malformed { offset: val_start });
        }
        members.push(MemberSpan {
            key_start,
            key_end,
            val_start,
            val_end,
        });
        i = val_end;
    }
    Ok(members)
}

/// Append `value` as a JSON string literal (`"` + RFC 8259 escaping + `"`).
/// This is the only place mutation (b) produces bytes.
fn append_json_string(out: &mut Vec<u8>, s: &str) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    out.push(b'"');
    for c in s.chars() {
        match c {
            '"' => out.extend_from_slice(b"\\\""),
            '\\' => out.extend_from_slice(b"\\\\"),
            '\n' => out.extend_from_slice(b"\\n"),
            '\r' => out.extend_from_slice(b"\\r"),
            '\t' => out.extend_from_slice(b"\\t"),
            '\u{0008}' => out.extend_from_slice(b"\\b"),
            '\u{000C}' => out.extend_from_slice(b"\\f"),
            c if (c as u32) < 0x20 => {
                let cp = c as u32;
                out.extend_from_slice(b"\\u00");
                out.push(HEX[((cp >> 4) & 0xF) as usize]);
                out.push(HEX[(cp & 0xF) as usize]);
            }
            c => {
                let mut buf = [0u8; 4];
                out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    out.push(b'"');
}

/// Byte span of a top-level object member (separator comma excluded;
/// `key_*` includes both quotes).
struct MemberSpan {
    key_start: usize,
    key_end: usize,
    val_start: usize,
    val_end: usize,
}

#[inline]
fn is_ws(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r')
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && is_ws(b[i]) {
        i += 1;
    }
    i
}

/// Validate the escape sequence starting at `b[at]` (`\"` `\\` `\uXXXX` and
/// other RFC 8259 legal forms). `at` points at the backslash itself.
fn check_escape(b: &[u8], at: usize) -> Result<(), RawEditError> {
    let next = *b
        .get(at + 1)
        .ok_or(RawEditError::Malformed { offset: at })?;
    match next {
        b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't' => Ok(()),
        b'u' => {
            for k in 2..6 {
                let h = *b
                    .get(at + k)
                    .ok_or(RawEditError::Malformed { offset: at })?;
                if !h.is_ascii_hexdigit() {
                    return Err(RawEditError::Malformed { offset: at });
                }
            }
            Ok(())
        }
        _ => Err(RawEditError::Malformed { offset: at }),
    }
}

/// Scan from the opening quote at `i` past the closing quote; return the index
/// just after the closing quote. Escapes are validated strictly; raw control
/// characters are tolerated leniently (they do not affect span determination).
/// Multi-byte UTF-8 needs no special casing: bytes >= 0x80 never collide with
/// any structural ASCII byte (`" \` `{}[],:`).
fn scan_json_string(b: &[u8], i: usize) -> Result<usize, RawEditError> {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => {
                check_escape(b, j)?;
                j += 2;
            }
            b'"' => return Ok(j + 1),
            _ => j += 1,
        }
    }
    Err(RawEditError::Malformed { offset: i })
}

/// Decode a JSON string literal (the range including quotes) into a `String`
/// for key comparison. Key matching must go through decoded semantic equality:
/// `"router_\u006deta"` and `"router_meta"` are the same key.
fn decode_json_string(b: &[u8], start: usize, end: usize) -> Result<String, RawEditError> {
    let mut out = String::with_capacity(end - start);
    let mut j = start + 1;
    while j < end - 1 {
        match b[j] {
            b'\\' => {
                check_escape(b, j)?;
                match b[j + 1] {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{0008}'),
                    b'f' => out.push('\u{000C}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let cp = u32::from_str_radix(
                            std::str::from_utf8(&b[j + 2..j + 6])
                                .map_err(|_| RawEditError::Malformed { offset: j })?,
                            16,
                        )
                        .map_err(|_| RawEditError::Malformed { offset: j })?;
                        let ch = char::from_u32(cp).ok_or(RawEditError::Malformed { offset: j })?;
                        out.push(ch);
                        j += 4;
                    }
                    _ => unreachable!("check_escape already filtered this branch"),
                }
                j += 2;
            }
            c => {
                // Multi-byte UTF-8 goes into the buffer as-is; invalid UTF-8 →
                // Malformed.
                let step = utf8_step(c);
                let s = std::str::from_utf8(&b[j..j + step])
                    .map_err(|_| RawEditError::Malformed { offset: j })?;
                out.push_str(s);
                j += step;
            }
        }
    }
    Ok(out)
}

/// UTF-8 sequence length from the leading byte; ASCII (including structural
/// bytes) is always 1.
#[inline]
fn utf8_step(c: u8) -> usize {
    match c {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        0xF0..=0xF7 => 4,
        // Illegal leading byte (0x80..=0xBF / >= 0xF8): treat as 1; the
        // subsequent from_utf8 will report the error.
        _ => 1,
    }
}

/// Scan one member value, returning the index just past the value's end.
/// Objects/arrays use a bracket stack to guarantee pairing; raw values
/// (number/true/false/null) use serde_json **for validation only** (it never
/// produces output).
fn scan_value(b: &[u8], i: usize) -> Result<usize, RawEditError> {
    match b[i] {
        b'"' => scan_json_string(b, i),
        b'{' | b'[' => {
            let mut stack: Vec<u8> = vec![b[i]];
            let mut j = i + 1;
            while j < b.len() {
                let c = b[j];
                if *stack.last().expect("stack is non-empty") == b'"' {
                    // Inside a string: only escapes and the closing quote
                    // count.
                    match c {
                        b'\\' => {
                            check_escape(b, j)?;
                            j += 2;
                            continue;
                        }
                        b'"' => {
                            stack.pop();
                        }
                        _ => {}
                    }
                } else {
                    match c {
                        b'"' => stack.push(b'"'),
                        b'{' | b'[' => stack.push(c),
                        b'}' | b']' => {
                            let open = stack.pop().expect("stack is non-empty");
                            let expect_close = if open == b'{' { b'}' } else { b']' };
                            if c != expect_close {
                                return Err(RawEditError::Malformed { offset: j });
                            }
                            if stack.is_empty() {
                                return Ok(j + 1);
                            }
                        }
                        _ => {}
                    }
                }
                j += 1;
            }
            Err(RawEditError::Malformed { offset: i })
        }
        _ => {
            // Raw value: scan to the first delimiter (whitespace/comma/closing
            // bracket/EOF).
            let mut j = i;
            while j < b.len() && !is_ws(b[j]) && b[j] != b',' && b[j] != b'}' {
                j += 1;
            }
            if j == i {
                return Err(RawEditError::Malformed { offset: i }); // empty value, e.g. {"a":}
            }
            serde_json::from_slice::<serde_json::Value>(&b[i..j])
                .map_err(|_| RawEditError::Malformed { offset: i })?;
            Ok(j)
        }
    }
}

/// Single-pass scan of all member spans of the top-level object; also
/// validates the structure of the entire body.
fn scan_top_level_members(b: &[u8]) -> Result<Vec<MemberSpan>, RawEditError> {
    let n = b.len();
    let mut i = skip_ws(b, 0);
    if i >= n {
        return Err(RawEditError::EmptyBody);
    }
    if b[i] != b'{' {
        return Err(RawEditError::NotTopLevelObject { first_byte: b[i] });
    }
    i = skip_ws(b, i + 1);

    let mut members = Vec::new();
    loop {
        if i < n && b[i] == b'}' {
            i += 1;
            break;
        }
        // A member key must be a string.
        if i >= n || b[i] != b'"' {
            return Err(RawEditError::Malformed { offset: i.min(n) });
        }
        let key_start = i;
        i = scan_json_string(b, i)?;
        let key_end = i;
        i = skip_ws(b, i);
        if i >= n || b[i] != b':' {
            return Err(RawEditError::Malformed { offset: i.min(n) });
        }
        i = skip_ws(b, i + 1);
        if i >= n {
            return Err(RawEditError::Malformed { offset: n });
        }
        let val_start = i;
        let val_end = scan_value(b, i)?;
        i = skip_ws(b, val_end);
        members.push(MemberSpan {
            key_start,
            key_end,
            val_start,
            val_end,
        });
        if i < n && b[i] == b',' {
            i = skip_ws(b, i + 1);
            // Trailing comma (`{"a":1,}`) is illegal: the comma must be
            // immediately followed by the next member.
            if i < n && b[i] == b'}' {
                return Err(RawEditError::Malformed { offset: i });
            }
            continue;
        }
        if i < n && b[i] == b'}' {
            i += 1;
            break;
        }
        return Err(RawEditError::Malformed { offset: i.min(n) });
    }
    // Only trailing whitespace is allowed after the closing brace.
    let tail = skip_ws(b, i);
    if tail != n {
        return Err(RawEditError::Malformed { offset: tail });
    }
    Ok(members)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shortcut: remove keys and assert byte-equality against the expected
    /// output.
    fn assert_remove(input: &str, keys: &[&str], expected: &str) {
        let raw = RawBody::new(input.as_bytes().to_vec());
        let out = raw
            .remove_top_level_keys(keys)
            .unwrap_or_else(|e| panic!("unexpected err: {e:?}"));
        assert_eq!(
            String::from_utf8_lossy(out.as_bytes()),
            expected,
            "input: {input}"
        );
    }

    // 1. Structural chars { } , : inside a string value must not be mistaken
    //    for structure.
    #[test]
    fn string_value_with_structural_chars() {
        assert_remove(
            r#"{"model":"a{b}c:d,e","router_meta":{}}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"model":"a{b}c:d,e"}"#,
        );
    }

    // 2. Escaped quotes and backslashes are preserved verbatim.
    #[test]
    fn escaped_quote_and_backslash_preserved() {
        assert_remove(
            r#"{"s":"a\"b\\c\"d","router_meta":1}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"s":"a\"b\\c\"d"}"#,
        );
    }

    // 3. \uXXXX forms are preserved verbatim (no decode → re-encode).
    #[test]
    fn unicode_escape_form_preserved() {
        assert_remove(
            r#"{"s":"\u0041\u4e2d\ud83d\ude00","router_meta":null}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"s":"\u0041\u4e2d\ud83d\ude00"}"#,
        );
    }

    // 4. Nested object/array brackets stay balanced: only the top level is
    //    deleted; same-named nested keys are untouched.
    #[test]
    fn nested_brackets_balance_and_only_top_level_removed() {
        assert_remove(
            "{\"router_meta\":{\"x\":[1,{\"y\":\"},]\"}],\"z\":[[]]},\"a\":1}",
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"a":1}"#,
        );
        // A nested router_meta is a client field (CONF-11), not a top-level
        // echo → not removed.
        assert_remove(
            r#"{"a":{"router_meta":1},"b":2}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"a":{"router_meta":1},"b":2}"#,
        );
    }

    // 5. All value forms preserved byte-for-byte: 1e-9 / leading minus sign /
    //    big integer / bool / null / [] / {} / empty string.
    #[test]
    fn all_value_forms_preserved() {
        assert_remove(
            r#"{"n1":1e-9,"n2":-0.5,"n3":123456789012345678901234567890,"t":true,"f":false,"z":null,"arr":[],"obj":{},"es":"","router_meta":0}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"n1":1e-9,"n2":-0.5,"n3":123456789012345678901234567890,"t":true,"f":false,"z":null,"arr":[],"obj":{},"es":""}"#,
        );
    }

    // 6. Key absent → no-op with output byte-identical to input.
    #[test]
    fn missing_key_is_byte_identical_noop() {
        let input = r#"{"a":1,"b":[2,3]}"#;
        let raw = RawBody::new(input.as_bytes().to_vec());
        let out = raw
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        assert_eq!(out.as_bytes(), input.as_bytes());
    }

    // 7. keys = [] → identity.
    #[test]
    fn empty_key_list_is_identity() {
        let input = r#"{"router_meta":1,"a":2}"#;
        let raw = RawBody::new(input.as_bytes().to_vec());
        let out = raw.remove_top_level_keys(&[]).unwrap();
        assert_eq!(out.as_bytes(), input.as_bytes());
    }

    // 8. Idempotent: two consecutive calls == one (including comma-swallowing
    //    boundaries).
    #[test]
    fn idempotent_double_remove() {
        let input = "{\n  \"model\": \"m\",\n  \"router_meta\": {\"a\": [1, {\"b\": \"},\"}]},\n  \"x\": 1e-9\n}";
        let raw = RawBody::new(input.as_bytes().to_vec());
        let once = raw
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        let twice = once
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        assert_eq!(once.as_bytes(), twice.as_bytes());
    }

    // 9. Non-object top level → Err(NotTopLevelObject) (one pinned case each
    //    for array/string/number/scalar).
    #[test]
    fn top_level_non_object_rejected() {
        for input in ["[1,2]", "\"str\"", "42", "true", "null"] {
            let raw = RawBody::new(input.as_bytes().to_vec());
            assert!(
                matches!(
                    raw.remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS),
                    Err(RawEditError::NotTopLevelObject { .. })
                ),
                "expected NotTopLevelObject for {input}"
            );
        }
    }

    // 10. Empty body / whitespace-only → Err(EmptyBody).
    #[test]
    fn empty_or_whitespace_only_rejected() {
        for input in ["", "   ", " \n\r\t "] {
            let raw = RawBody::new(input.as_bytes().to_vec());
            assert_eq!(
                raw.remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS),
                Err(RawEditError::EmptyBody)
            );
        }
    }

    // 11. Malformed structure → Err(Malformed): empty value / trailing comma /
    //     trailing content / unclosed / mismatched bracket / illegal escape.
    #[test]
    fn malformed_bodies_rejected() {
        for input in [
            r#"{"a":}"#,
            r#"{"a":1,}"#,
            r#"{"a":1} x"#,
            r#"{"a":"b"#,
            r#"{"a":{"b":1}"#,
            r#"{"a":[1}"#,
            r#"{"a":1"#,
            r#"{"a":"b\q"}"#,
            r#"{"a":"\u00zz"}"#,
        ] {
            let raw = RawBody::new(input.as_bytes().to_vec());
            assert!(
                matches!(
                    raw.remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS),
                    Err(RawEditError::Malformed { .. })
                ),
                "expected Malformed for {input}"
            );
        }
    }

    // 12. Key order and nested field order completely unchanged (multiple
    //     fields + deleting a middle member).
    #[test]
    fn key_and_nested_order_unchanged() {
        assert_remove(
            r#"{"z":1,"a":{"deep":[3,2,{"k":"v"}]},"router_meta":0,"m":"x","b":true}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"z":1,"a":{"deep":[3,2,{"k":"v"}]},"m":"x","b":true}"#,
        );
    }

    // 13. Trailing newline / CRLF / multi-byte UTF-8 values preserved
    //     verbatim.
    #[test]
    fn trailing_newline_crlf_and_utf8_preserved() {
        assert_remove(
            "{\n  \"a\": \"中文🚀\",\n  \"router_meta\": 1\n}\r\n",
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            "{\n  \"a\": \"中文🚀\"\n}\r\n",
        );
    }

    // 14. Whitespace between members and comma swallowing for a deleted first
    //     member: the output is still valid JSON and all other bytes are
    //     unchanged.
    #[test]
    fn whitespace_around_members_and_first_member_removal() {
        assert_remove(
            "{ \"router_meta\":1 , \"a\":2 , \"b\":3 }",
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            "{  \"a\":2 , \"b\":3 }", // what is deleted is `, "router_meta":1`: whitespace before the key stays, the leading space is kept
        );
        assert_remove(
            "{\"a\":1 ,\n\t\"router_meta\":2 , \"b\":3}",
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            "{\"a\":1  , \"b\":3}",
        );
    }

    // 15. All members removed: empty object / braces containing only leftover
    //     whitespace.
    #[test]
    fn removing_all_members_yields_empty_object() {
        assert_remove(r#"{"router_meta":1}"#, ROUTER_OWNED_TOP_LEVEL_KEYS, "{}");
        assert_remove(
            r#"{"router_meta":1, "router_meta":2}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            "{}",
        );
    }

    // 16. Escaped key form matches semantically: `"router_\u006deta"` and
    //     `"router_meta"` are the same key.
    #[test]
    fn escaped_key_form_still_matches() {
        assert_remove(
            r#"{"a":1,"router_\u006deta":2}"#,
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            r#"{"a":1}"#,
        );
    }

    // 17. Whitespace before `{` is preserved (byte fidelity includes the part
    //     before the first byte of the object).
    #[test]
    fn leading_whitespace_preserved() {
        assert_remove(
            "  {\"a\":1,\"router_meta\":2}",
            ROUTER_OWNED_TOP_LEVEL_KEYS,
            "  {\"a\":1}",
        );
    }

    // 19. Deletion position matrix: adjacent deletions must not leave dangling
    //     commas (the historical regression; 5 probe-table cases + extras). `keys` is
    //     passed directly as a parameter (the two-key shape simulates the
    //     future whitelist in spec §2) without changing the ROUTER_OWNED
    //     constant.
    fn deletion_matrix() -> Vec<(&'static str, Vec<&'static str>, &'static str)> {
        let two = vec!["router_meta", "routing_preference"];
        vec![
            // The orchestrator's 5 probe-table cases.
            (
                r#"{"routing_preference":{},"router_meta":{},"messages":[]}"#,
                two.clone(),
                r#"{"messages":[]}"#,
            ),
            (
                r#"{"router_meta":1,"router_meta":2,"x":3}"#,
                two.clone(),
                r#"{"x":3}"#,
            ),
            (
                r#"{"messages":[],"routing_preference":1,"router_meta":2}"#,
                two.clone(),
                r#"{"messages":[]}"#,
            ),
            (
                r#"{"router_meta":1,"messages":[],"routing_preference":2}"#,
                two.clone(),
                r#"{"messages":[]}"#,
            ),
            (
                r#"{"router_meta":1,"routing_preference":2}"#,
                two.clone(),
                "{}",
            ),
            // First member deleted with the next member retained.
            (r#"{"router_meta":1,"a":2}"#, two.clone(), r#"{"a":2}"#),
            // Three consecutive owned keys.
            (
                r#"{"router_meta":1,"routing_preference":2,"router_meta":3,"a":4}"#,
                two.clone(),
                r#"{"a":4}"#,
            ),
            // Permutations with the owned key at the head / middle / tail.
            (
                r#"{"router_meta":1,"a":1,"b":2}"#,
                two.clone(),
                r#"{"a":1,"b":2}"#,
            ),
            (
                r#"{"a":1,"router_meta":2,"b":3}"#,
                two.clone(),
                r#"{"a":1,"b":3}"#,
            ),
            (
                r#"{"a":1,"b":2,"router_meta":3}"#,
                two.clone(),
                r#"{"a":1,"b":2}"#,
            ),
            // Adjacent deletion with whitespace: comma swallowing includes the
            // whitespace between member and comma.
            (
                r#"{ "routing_preference" : 1 , "router_meta" : 2 , "messages" : [] }"#,
                two.clone(),
                r#"{  "messages" : [] }"#,
            ),
        ]
    }

    #[test]
    fn deletion_position_matrix() {
        for (input, keys, expected) in deletion_matrix() {
            assert_remove(input, &keys, expected);
        }
    }

    // 19b. Invariant guard: every `Ok` output in the matrix must be valid
    //      JSON — the generic detector for dangling-comma-style bugs
    //      (`output_is_valid_json` covers only a single body, this covers the
    //      whole matrix).
    #[test]
    fn deletion_matrix_outputs_are_valid_json() {
        for (input, keys, _) in deletion_matrix() {
            let raw = RawBody::new(input.as_bytes().to_vec());
            let out = raw
                .remove_top_level_keys(&keys)
                .unwrap_or_else(|e| panic!("matrix case must be Ok: {input}, err {e:?}"));
            assert!(
                serde_json::from_slice::<serde_json::Value>(out.as_bytes()).is_ok(),
                "output is not valid JSON for input: {input}"
            );
        }
    }

    // 19c. Deletion at three scales: whitelist of 1/2/3 keys × 3/5/8 members.
    #[test]
    fn deletion_matrix_scales() {
        let k1 = vec!["router_meta"];
        let k2 = vec!["router_meta", "routing_preference"];
        let k3 = vec!["router_meta", "routing_preference", "router_hint"];
        // 3 members.
        assert_remove(r#"{"router_meta":1,"a":1,"b":2}"#, &k1, r#"{"a":1,"b":2}"#);
        assert_remove(
            r#"{"router_meta":1,"routing_preference":2,"a":1}"#,
            &k2,
            r#"{"a":1}"#,
        );
        // 5 members.
        assert_remove(
            r#"{"a":0,"router_meta":1,"routing_preference":2,"router_hint":3,"b":4}"#,
            &k3,
            r#"{"a":0,"b":4}"#,
        );
        assert_remove(
            r#"{"router_meta":1,"routing_preference":2,"a":0,"router_hint":3,"b":4}"#,
            &k3,
            r#"{"a":0,"b":4}"#,
        );
        // 8 members.
        assert_remove(
            r#"{"a":0,"router_meta":1,"b":2,"routing_preference":3,"c":4,"router_hint":5,"d":6,"e":7}"#,
            &k3,
            r#"{"a":0,"b":2,"c":4,"d":6,"e":7}"#,
        );
    }

    // 19d. Boundary behavior pinned (DESIGN §12.3.1): BOM → rejected.
    //      Stripping a BOM would be a rewrite outside the whitelist (AGENTS
    //      hard constraint 1), so it is rejected as "top level is not an
    //      object" and the caller maps it to a 400.
    #[test]
    fn bom_rejected_as_not_top_level_object() {
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(br#"{"a":1}"#);
        assert_eq!(
            RawBody::new(b).remove_top_level_keys(&["router_meta"]),
            Err(RawEditError::NotTopLevelObject { first_byte: 0xEF })
        );
    }

    // 19e. Boundary behavior pinned: leading-zero number `01` is not RFC 8259
    //      number grammar → Malformed (raw value fragments are vetted by the
    //      serde_json validator, which does not accept ambiguous number
    //      forms).
    #[test]
    fn leading_zero_number_rejected_as_malformed() {
        let raw = RawBody::new(br#"{"a":01}"#.to_vec());
        assert!(matches!(
            raw.remove_top_level_keys(&[]),
            Err(RawEditError::Malformed { .. })
        ));
    }

    // 19f. Boundary behavior pinned: invalid UTF-8 inside a string **value**
    //      → `Ok` with byte-for-byte passthrough. The router does not
    //      interpret value contents (byte boundary first, the upstream decides);
    //      keys however must be decodable to compare semantically against the
    //      whitelist, so invalid UTF-8 in a key is still Malformed — the
    //      asymmetry is intentional.
    #[test]
    fn invalid_utf8_in_string_value_passthrough() {
        let input: Vec<u8> = b"{\"s\":\"\xFF\xFE\",\"router_meta\":1}".to_vec();
        let raw = RawBody::new(input);
        let out = raw.remove_top_level_keys(&["router_meta"]).unwrap();
        assert_eq!(out.as_bytes(), b"{\"s\":\"\xFF\xFE\"}");
    }

    // 18. The deletion output itself is valid JSON (a parse assertion over the
    //     post-edit bytes).
    #[test]
    fn output_is_valid_json() {
        let raw = RawBody::new(
            "{\"router_meta\":{\"x\":[1,{\"y\":\"},]\"}]},\"model\":\"m\",\"n\":1e-09}"
                .as_bytes()
                .to_vec(),
        );
        let out = raw
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(out.as_bytes()).unwrap();
        assert_eq!(v["model"], "m");
        assert!(v.get("router_meta").is_none());
    }

    // === set_top_level_string (DESIGN §12.3.1 mutation (b)) ===

    /// Shortcut: replace and assert byte-equality against the expected
    /// output, then run the generic invariants (valid JSON, diff-set empty
    /// outside the value span, idempotence).
    fn assert_set(input: &str, key: &str, value: &str, expected: &str) {
        let raw = RawBody::new(input.as_bytes().to_vec());
        let out = raw
            .set_top_level_string(key, value)
            .unwrap_or_else(|e| panic!("unexpected err: {e:?}"));
        assert_eq!(
            out.as_ref(),
            expected.as_bytes(),
            "input: {input}, key: {key}, value: {value}"
        );
        assert_set_invariants(input, key, value);
    }

    /// Independent oracle: locate the member's value span via the public
    /// spans API, splice in a locally-encoded JSON string, and require the
    /// implementation's bytes to equal that splice. This checks the diff-set
    /// is empty outside the value span without trusting the implementation's
    /// own notion of its output.
    fn assert_set_invariants(input: &str, key: &str, value: &str) {
        let raw = RawBody::new(input.as_bytes().to_vec());
        let out = raw.set_top_level_string(key, value).unwrap();

        // 1. Any Ok output must be valid JSON (the generic detector).
        let parsed: serde_json::Value = serde_json::from_slice(&out)
            .unwrap_or_else(|e| panic!("output is not valid JSON: {e}, input: {input}"));

        // 2. The decodable value of the member equals the target string
        //    (serde_json's map keeps the last duplicate, same rule as ours).
        assert_eq!(
            parsed.get(key).and_then(|v| v.as_str()),
            Some(value),
            "decoded member value mismatch, input: {input}"
        );

        // 3. Diff-set empty outside the value span: rebuild the expected
        //    bytes from the input by replacing only the (last) member's value
        //    span with a locally encoded string.
        let spans = raw.top_level_member_spans().unwrap();
        let (vs, ve) = spans
            .iter()
            .rev()
            .find(|(k, _, _)| k == key)
            .map(|(_, s, e)| (*s, *e))
            .expect("oracle requires the key to be present");
        let mut oracle = Vec::with_capacity(input.len());
        oracle.extend_from_slice(&input.as_bytes()[..vs]);
        oracle.extend_from_slice(oracle_encode_json_string(value).as_bytes());
        oracle.extend_from_slice(&input.as_bytes()[ve..]);
        assert_eq!(
            out.as_ref(),
            oracle.as_slice(),
            "bytes outside the value span changed, input: {input}"
        );

        // 4. Idempotence: applying the same set to the output is a byte-level
        //    no-op.
        let raw2 = RawBody::new(out.to_vec());
        let out2 = raw2.set_top_level_string(key, value).unwrap();
        assert!(
            matches!(out2, Cow::Borrowed(_)),
            "second application must be Borrowed"
        );
        assert_eq!(out2.as_ref(), out.as_ref());
    }

    /// RFC 8259 string encoding, written independently of the implementation
    /// so an escaping bug cannot hide in both sides of the assertion.
    fn oracle_encode_json_string(s: &str) -> String {
        let mut out = String::with_capacity(s.len() + 2);
        out.push('"');
        for c in s.chars() {
            match c {
                '"' => out.push_str("\\\""),
                '\\' => out.push_str("\\\\"),
                '\n' => out.push_str("\\n"),
                '\r' => out.push_str("\\r"),
                '\t' => out.push_str("\\t"),
                '\u{0008}' => out.push_str("\\b"),
                '\u{000C}' => out.push_str("\\f"),
                c if (c as u32) < 0x20 => {
                    out.push_str(&format!("\\u{:04x}", c as u32));
                }
                c => out.push(c),
            }
        }
        out.push('"');
        out
    }

    // 20. Happy path: only the value span moves; everything else — position,
    //     separators, whitespace, trailing newline — is the input's.
    #[test]
    fn set_replaces_only_the_value_span() {
        assert_set(
            r#"{"model":"gpt-4o","messages":[{"role":"user","content":"hi"}]}"#,
            "model",
            "deepseek-chat",
            r#"{"model":"deepseek-chat","messages":[{"role":"user","content":"hi"}]}"#,
        );
        // Middle position, pretty-printed whitespace and a trailing newline.
        assert_set(
            "{\n  \"a\": 1,\n  \"model\": \"old\",\n  \"b\": [2, 3]\n}\n",
            "model",
            "new",
            "{\n  \"a\": 1,\n  \"model\": \"new\",\n  \"b\": [2, 3]\n}\n",
        );
        // Sole member, CRLF line endings preserved around it.
        assert_set(
            "{\r\n  \"model\" : \"x\"\r\n}",
            "model",
            "y",
            "{\r\n  \"model\" : \"y\"\r\n}",
        );
        // Leading whitespace before the brace survives.
        assert_set("  {\"model\":\"a\"}", "model", "b", "  {\"model\":\"b\"}");
    }

    // 21. Escape parity: a value containing quote/backslash/newline is
    //     encoded per RFC 8259 (hand-written expectations, not the oracle's).
    #[test]
    fn set_escapes_the_new_value() {
        assert_set(
            r#"{"model":"a"}"#,
            "model",
            "say \"hi\"",
            r#"{"model":"say \"hi\""}"#,
        );
        assert_set(
            r#"{"model":"a"}"#,
            "model",
            "back\\slash",
            r#"{"model":"back\\slash"}"#,
        );
        assert_set(
            "{\"model\":\"a\"}",
            "model",
            "line1\nline2\ttab\rret",
            "{\"model\":\"line1\\nline2\\ttab\\rret\"}",
        );
        // Control characters fall back to \u00XX.
        assert_set(
            "{\"model\":\"a\"}",
            "model",
            "\u{0001}\u{001F}",
            "{\"model\":\"\\u0001\\u001f\"}",
        );
        // Escape oddity: a value that is itself a backslash-escape-looking
        // string must not be double-processed.
        assert_set(
            r#"{"model":"a"}"#,
            "model",
            "\\u0041",
            r#"{"model":"\\u0041"}"#,
        );
        // Old value's escapes are irrelevant to the new span.
        assert_set(
            r#"{"model":"old \"quoted\" \\ value"}"#,
            "model",
            "plain",
            r#"{"model":"plain"}"#,
        );
    }

    // 22. Multi-byte UTF-8 in the new value goes through unescaped.
    #[test]
    fn set_multi_byte_utf8_value() {
        assert_set(
            r#"{"model":"a"}"#,
            "model",
            "deepseek-中文🚀-v2",
            "{\"model\":\"deepseek-中文🚀-v2\"}",
        );
        // Multi-byte UTF-8 elsewhere in the body is untouched.
        assert_set(
            "{\"model\":\"a\",\"s\":\"中文🚀\"}",
            "model",
            "b",
            "{\"model\":\"b\",\"s\":\"中文🚀\"}",
        );
    }

    // 23. Nested same-named keys are NOT replaced — only the top level.
    #[test]
    fn set_touches_only_top_level_not_nested_same_key() {
        assert_set(
            r#"{"model":"top","inner":{"model":"nested"},"arr":[{"model":"in-array"}]}"#,
            "model",
            "new",
            r#"{"model":"new","inner":{"model":"nested"},"arr":[{"model":"in-array"}]}"#,
        );
        // The target string appearing inside another member's value.
        assert_set(
            r#"{"model":"a","x":"model"}"#,
            "model",
            "b",
            r#"{"model":"b","x":"model"}"#,
        );
    }

    // 24. Duplicate top-level keys: the LAST occurrence is replaced (the
    //     decodable value), the earlier duplicate stays verbatim.
    #[test]
    fn set_duplicate_top_level_keys_replaces_last_occurrence() {
        assert_set(
            r#"{"model":"a","x":1,"model":"b"}"#,
            "model",
            "new",
            r#"{"model":"a","x":1,"model":"new"}"#,
        );
        // Three duplicates; the last one wins.
        assert_set(
            r#"{"model":"a","model":"b","model":"c"}"#,
            "model",
            "new",
            r#"{"model":"a","model":"b","model":"new"}"#,
        );
        // Only one occurrence, at the tail.
        assert_set(
            r#"{"x":1,"model":"old"}"#,
            "model",
            "new",
            r#"{"x":1,"model":"new"}"#,
        );
    }

    // 25. Absent key → Err(TopLevelKeyAbsent), never a silent insertion.
    #[test]
    fn set_absent_key_is_error_never_insertion() {
        let raw = RawBody::new(br#"{"messages":[]}"#.to_vec());
        assert_eq!(
            raw.set_top_level_string("model", "x"),
            Err(RawEditError::TopLevelKeyAbsent)
        );
        // Different key spelling is still absent.
        let raw = RawBody::new(br#"{"model":"a"}"#.to_vec());
        assert_eq!(
            raw.set_top_level_string("Model", "x"),
            Err(RawEditError::TopLevelKeyAbsent)
        );
        // Empty object.
        let raw = RawBody::new(br#"{}"#.to_vec());
        assert_eq!(
            raw.set_top_level_string("model", "x"),
            Err(RawEditError::TopLevelKeyAbsent)
        );
    }

    // 26. Present member whose value is not a JSON string → error, with the
    //     value's first byte for the caller's 400.
    #[test]
    fn set_non_string_value_is_error() {
        for (input, first) in [
            (r#"{"model":42}"#, b'4'),
            (r#"{"model":true}"#, b't'),
            (r#"{"model":null}"#, b'n'),
            (r#"{"model":[]}"#, b'['),
            (r#"{"model":{}}"#, b'{'),
            (r#"{"model":-1.5e3}"#, b'-'),
        ] {
            let raw = RawBody::new(input.as_bytes().to_vec());
            assert_eq!(
                raw.set_top_level_string("model", "x"),
                Err(RawEditError::TopLevelValueNotString { first_byte: first }),
                "input: {input}"
            );
        }
    }

    // 27. Value equal to the current value → Cow::Borrowed, byte-identical.
    #[test]
    fn set_equal_value_is_borrowed_noop() {
        let input = r#"{"model":"same","x":1}"#;
        let raw = RawBody::new(input.as_bytes().to_vec());
        let out = raw.set_top_level_string("model", "same").unwrap();
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.as_ref(), input.as_bytes());
        // Equal even when the old value uses \uXXXX escapes the new value
        // does not: decoded equality, bytes untouched.
        let raw = RawBody::new(br#"{"model":"sam\u0065"}"#.to_vec());
        let out = raw.set_top_level_string("model", "same").unwrap();
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.as_ref(), br#"{"model":"sam\u0065"}"#.as_ref());
    }

    // 28. Empty value and a very long value.
    #[test]
    fn set_empty_and_long_values() {
        assert_set(r#"{"model":"a"}"#, "model", "", r#"{"model":""}"#);
        let long = "m".repeat(100_000);
        let expected = format!(r#"{{"model":"{long}"}}"#);
        assert_set(r#"{"model":"a"}"#, "model", &long, &expected);
    }

    // 29. Structural chars in the old value and its neighbors: braces,
    //     colons and commas inside strings must not confuse the scanner.
    #[test]
    fn set_with_structural_chars_in_strings() {
        assert_set(
            r#"{"s":"a{b}c:d,e","model":"old"}"#,
            "model",
            "new",
            r#"{"s":"a{b}c:d,e","model":"new"}"#,
        );
        // Old value contains an escaped structural char sequence.
        assert_set(
            r#"{"model":"}\"","x":"[{\"k\":1}]"}"#,
            "model",
            "n",
            r#"{"model":"n","x":"[{\"k\":1}]"}"#,
        );
    }

    // 30. Key matched semantically: an escaped key form is the same key.
    #[test]
    fn set_escaped_key_form_still_matches() {
        assert_set(
            r#"{"a":1,"mod\u0065l":"old"}"#,
            "model",
            "new",
            r#"{"a":1,"mod\u0065l":"new"}"#,
        );
    }

    // 31. Structural failures reuse the deletion scanner's errors.
    #[test]
    fn set_malformed_and_non_object_bodies_rejected() {
        for input in ["", "   ", "[1,2]", "\"str\"", "42", "true", "null"] {
            let raw = RawBody::new(input.as_bytes().to_vec());
            assert!(
                raw.set_top_level_string("model", "x").is_err(),
                "expected err for {input:?}"
            );
        }
        let raw = RawBody::new(br#"{"model":"a"#.to_vec());
        assert!(matches!(
            raw.set_top_level_string("model", "x"),
            Err(RawEditError::Malformed { .. })
        ));
        // BOM → NotTopLevelObject (same pinned boundary as deletion).
        let mut b = vec![0xEF, 0xBB, 0xBF];
        b.extend_from_slice(br#"{"model":"a"}"#);
        assert_eq!(
            RawBody::new(b).set_top_level_string("model", "x"),
            Err(RawEditError::NotTopLevelObject { first_byte: 0xEF })
        );
    }

    // 32. Round-trip composition with mutation (a): remove → set leaves
    //     every other byte alone (the §12.10.7 pipeline shape).
    #[test]
    fn set_after_remove_composes() {
        let input = r#"{"router_meta":1,"model":"alias/name","x":{"k":"v"}}"#;
        let raw = RawBody::new(input.as_bytes().to_vec());
        let cleaned = raw
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        let out = cleaned.set_top_level_string("model", "native-id").unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out),
            r#"{"model":"native-id","x":{"k":"v"}}"#
        );
    }

    // 33. Cross-check against the family oracle: set(model, m2) then remove
    //     == remove-then-set; order of the two mutations does not matter for
    //     disjoint keys.
    #[test]
    fn set_and_remove_commute_for_disjoint_keys() {
        let input = r#"{"model":"alias","a":1,"router_meta":2,"b":3}"#;
        let raw = RawBody::new(input.as_bytes().to_vec());
        let sr = raw
            .set_top_level_string("model", "native")
            .unwrap()
            .to_vec();
        let sr = RawBody::new(sr)
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        let rs = raw
            .remove_top_level_keys(ROUTER_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        let rs_clean = RawBody::new(rs.as_bytes().to_vec());
        let rs = rs_clean.set_top_level_string("model", "native").unwrap();
        assert_eq!(sr.as_bytes(), rs.as_ref());
    }

    // 34. Mutation self-check (card requirement): these assertions must fail
    //     for plausible wrong implementations. Verified by temporarily
    //     mutating the implementation (first-duplicate-wins, unescaped
    //     output) and observing red — see the card's completion notes.
    #[test]
    fn set_mutation_matrix_outputs_are_valid_json_and_minimal() {
        let cases: Vec<(&str, &str, &str)> = vec![
            (r#"{"model":"a","b":1}"#, "zzz", r#"{"model":"zzz","b":1}"#),
            (r#"{"b":1,"model":"a"}"#, "zzz", r#"{"b":1,"model":"zzz"}"#),
            (
                r#"{"model":"a","model":"b"}"#,
                "zzz",
                r#"{"model":"a","model":"zzz"}"#,
            ),
            (
                r#"{"x":{"model":"keep"},"model":"a"}"#,
                "zzz",
                r#"{"x":{"model":"keep"},"model":"zzz"}"#,
            ),
        ];
        for (input, value, expected) in cases {
            assert_set(input, "model", value, expected);
        }
    }
}
