//! Byte-faithful primitive `RawBody` (DESIGN §12.3.1, ADR-007 "span-faithful
//! forwarding", AGENTS hard constraints 1/2).
//!
//! The only permitted rewrite is removing router-owned top-level fields; every
//! other byte is preserved verbatim. The implementation is a **single-pass span
//! scanner** (tracking strings/escapes/bracket depth) that locates the byte
//! ranges of members to delete and excises those ranges — a **parse →
//! reserialize round trip is forbidden** (that is the most common way to break
//! the byte boundary). `serde_json` is used only as a **validator** for raw
//! value fragments (true/false/null/number), never to produce any outbound
//! bytes.
//!
//! Container deviation: the DESIGN sketch draws `RawBody(Bytes)`, but the
//! `bytes` crate is not in `router-core`'s dependency whitelist (§12.1: only
//! serde/serde_json), so `Vec<u8>` is used instead. The zero-copy outbound
//! conversion (`Bytes::from(vec)`) happens in the proxy layer in R2; semantics
//! are unaffected.

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

    /// The only permitted rewrite: delete whitelisted top-level keys, preserve
    /// every other byte verbatim.
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
        // deletions used to leave a dangling comma (R1-2c). A run consisting of
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
}

/// Byte span of a top-level object member (separator comma excluded;
/// `key_*` includes both quotes).
struct MemberSpan {
    key_start: usize,
    key_end: usize,
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
        let val_end = scan_value(b, i)?;
        i = skip_ws(b, val_end);
        members.push(MemberSpan {
            key_start,
            key_end,
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
    //     commas (R1-2c regression; 5 probe-table cases + extras). `keys` is
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
}
