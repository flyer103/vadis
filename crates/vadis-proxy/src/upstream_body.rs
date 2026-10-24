//! ADR-060 D2 (spec §6): a deterministic refusal carries a bounded,
//! redacted slice of the upstream's own error object in
//! `errors[].details.upstream_body` — so a refusal is diagnosable from
//! the trace alone (the audit's finding: six undiagnosable refusals in
//! one week whose `details` carried only vadis's own summary).
//!
//! It is a **response-side** record: the bytes are the upstream's error
//! body that the classifier already reads (spec §4.2's evidence clause
//! — no second read, no larger buffer, no new bound on the wire), and
//! no request byte, no forwarded byte and no prefix hash is touched, so
//! the byte boundary (constraint 1) is not engaged. The slice lives in
//! the **trace JSONL only** — never in an event row (ADR-009 item 3 /
//! ADR-011 item 8 keep bodies out of the store), and never in the
//! client-facing §8 body (the merge point below clones the failure
//! before it attaches).
//!
//! Four hard limits, not a promise (spec §6's table):
//!
//! | rule | value |
//! |---|---|
//! | capture | ≤ 1024 bytes of the body |
//! | fields | `message` / `type` / `code` / `param`, and `error.{…}` one level down; every other key is dropped |
//! | per-field bound | `message` ≤ 256 bytes, the others ≤ 64 bytes each, cut on a UTF-8 boundary (`truncated: true` says so) |
//! | redaction | the process's configured credential values, auth schemes, key shapes and key-adjacent runs — each replaced by `[redacted]` |
//!
//! An unparsable body is `shape: "text"` with the byte count and **no
//! bytes** — free text is never copied, which is what keeps "never a
//! full message body that could carry user content" a property of the
//! format rather than of a provider's manners. The member is present
//! **iff** a body arrived and something survived the bounds; absent
//! means *no body, or nothing retained*, never "the body was empty".
//!
//! [`capture`] is the **only** producer of the `upstream_body` value
//! and [`attach`] the only writer of the key — a call site cannot
//! construct the detail inline around this module, which is the
//! mechanism, not a convention.

use serde_json::{json, Value};

/// The capture bound (spec §6's table): 1024 bytes of the body, from
/// the classifier's own read.
pub(crate) const CAPTURE_BOUND_BYTES: usize = 1024;
/// The per-field bound for `message`.
const MESSAGE_BOUND_BYTES: usize = 256;
/// The per-field bound for `type` / `code` / `param`.
const OTHER_BOUND_BYTES: usize = 64;

const REDACTED: &str = "[redacted]";

/// The allowlisted top-level field names, in wire order.
const FIELDS: [&str; 4] = ["message", "type", "code", "param"];

/// Capture → allowlist → redact → bound, as one function (ADR-060 §3.2:
/// "the slice is one function"). `credentials` are the process's own
/// configured credential values — read for exact-match redaction and
/// never recorded.
pub(crate) fn capture(body: &[u8], credentials: &[&str]) -> Option<Value> {
    // The capture bound itself: never more than 1024 bytes are even
    // parsed (a body past the bound whose prefix does not parse is the
    // text shape, by construction).
    let capture = &body[..body.len().min(CAPTURE_BOUND_BYTES)];
    if capture.is_empty() {
        // No body arrived: the member is absent, which spec §6 defines
        // as exactly that.
        return None;
    }
    let Ok(v) = serde_json::from_slice::<Value>(capture) else {
        // Unparsable: the byte count and no bytes — free text is never
        // copied. The count is of the bytes held (the capture), the only
        // body this function may look at.
        return Some(json!({
            "shape": "text",
            "bytes": capture.len(),
        }));
    };
    let Some(obj) = v.as_object() else {
        // Parsable but not an object (a bare scalar): nothing an
        // allowlist could retain — absent, the same word as "nothing
        // survived the bounds".
        return None;
    };
    let mut truncated = false;
    let mut slice = serde_json::Map::new();
    for field in FIELDS {
        if let Some(text) = retained(
            obj.get(field),
            field == "message",
            credentials,
            &mut truncated,
        ) {
            slice.insert(field.to_string(), Value::String(text));
        }
    }
    // `error.{message,type,code,param}` one level down, when `error` is
    // an object: the body's own shape, so a reader maps it back without
    // a convention.
    if let Some(err) = obj.get("error").and_then(Value::as_object) {
        let mut nested = serde_json::Map::new();
        for field in FIELDS {
            if let Some(text) = retained(
                err.get(field),
                field == "message",
                credentials,
                &mut truncated,
            ) {
                nested.insert(field.to_string(), Value::String(text));
            }
        }
        if !nested.is_empty() {
            slice.insert("error".to_string(), Value::Object(nested));
        }
    }
    if slice.is_empty() {
        // A body that arrived and parsed, but nothing survived the
        // allowlist: absent — never "the body was empty".
        return None;
    }
    let mut out = serde_json::Map::new();
    out.insert("shape".to_string(), Value::String("json".to_string()));
    out.extend(slice);
    if truncated {
        // Spec §6's per-field bound row: the flag says a reader holds a
        // prefix.
        out.insert("truncated".to_string(), Value::Bool(true));
    }
    Some(Value::Object(out))
}

/// One allowlisted field: scalar → its string form, bounded and
/// redacted; anything else (absent, null, object, array) drops out.
fn retained(
    v: Option<&Value>,
    is_message: bool,
    credentials: &[&str],
    truncated: &mut bool,
) -> Option<String> {
    let v = v?;
    if v.is_null() || v.is_object() || v.is_array() {
        return None;
    }
    let text = match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if text.is_empty() {
        return None;
    }
    let redacted = redact(&text, credentials);
    let bound = if is_message {
        MESSAGE_BOUND_BYTES
    } else {
        OTHER_BOUND_BYTES
    };
    Some(cut_on_char_boundary(&redacted, bound, truncated))
}

/// Cut to ≤ `bound` bytes on a UTF-8 boundary, setting the flag when a
/// reader is left holding a prefix.
fn cut_on_char_boundary(s: &str, bound: usize, truncated: &mut bool) -> String {
    if s.len() <= bound {
        return s.to_string();
    }
    let mut end = bound;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    *truncated = true;
    s[..end].to_string()
}

/// The redaction set (ADR-060 §3.2 / spec §6's table), applied to each
/// retained string: (a) every configured credential's exact value;
/// (b) `Bearer` / `Basic` auth schemes; (c) key shapes `sk-…`, `sk_…`,
/// `pk-…` and `(api[-_]?key|token|secret|password)["']?\s*[:=]\s*"?<v>`;
/// (d) a ≥32-char token-char run adjacent to a key-ish label. Never a
/// header — no header value is captured anywhere in this module.
pub(crate) fn redact(s: &str, credentials: &[&str]) -> String {
    // (a) exact credential values, no guessing: a value-level rule.
    let mut out = s.to_string();
    for cred in credentials.iter().filter(|c| !c.is_empty()) {
        if out.contains(cred) {
            out = out.replace(cred, REDACTED);
        }
    }
    redact_key_shapes(&out)
}

/// Is `c` a token-char (the run alphabet of rules (c)/(e))?
fn is_token_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-')
}

/// The key-ish labels of rules (d)/(e), longest first so `api-key` is
/// not eaten by `key`.
const LABELS: [&str; 6] = [
    "password", "api-key", "api_key", "apikey", "secret", "token",
];

/// Case-insensitive ASCII match of `word` at `lower[i..]`.
fn word_at(lower: &[u8], i: usize, word: &str) -> bool {
    lower[i..].starts_with(word.as_bytes())
}

/// Does the tail of `out` (what has already been emitted) sit beside a
/// key-ish label — rule (e)'s "adjacent to a key-ish label"?
fn beside_keyish_label(out: &str) -> bool {
    let tail: String = out
        .chars()
        .rev()
        .take(24)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let tail = tail.to_ascii_lowercase();
    [
        "key",
        "token",
        "secret",
        "password",
        "authorization",
        "bearer",
        "basic",
    ]
    .iter()
    .any(|l| tail.contains(l))
}

fn redact_key_shapes(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let lower: Vec<u8> = s.chars().map(|c| c.to_ascii_lowercase() as u8).collect();
    let n = chars.len();
    let mut out = String::new();
    let mut i = 0;
    while i < n {
        // (b) auth schemes: `Bearer <x>` / `Basic <x>`.
        if word_at(&lower, i, "bearer ") || word_at(&lower, i, "basic ") {
            let scheme_len = if word_at(&lower, i, "bearer ") {
                "bearer ".len()
            } else {
                "basic ".len()
            };
            out.extend(chars[i..i + scheme_len].iter());
            let mut j = i + scheme_len;
            while j < n && chars[j].is_whitespace() {
                out.push(chars[j]);
                j += 1;
            }
            let start = j;
            while j < n && !chars[j].is_whitespace() {
                j += 1;
            }
            if j > start {
                out.push_str(REDACTED);
            }
            i = j;
            continue;
        }
        // (c) key shapes: a `sk-…` / `sk_…` / `pk-…` prefixed run — the
        // prefix plus at least five token chars, so ordinary words that
        // happen to start `sk-` survive.
        if word_at(&lower, i, "sk-") || word_at(&lower, i, "sk_") || word_at(&lower, i, "pk-") {
            let mut j = i + 3;
            while j < n && is_token_char(chars[j]) {
                j += 1;
            }
            if j - i >= 8 {
                out.push_str(REDACTED);
                i = j;
                continue;
            }
        }
        // (d) a key-ish label, optionally quoted, then `:` or `=`, then
        // the value run (unquoted or double-quoted).
        let mut label_hit: Option<usize> = None;
        for label in LABELS {
            if word_at(&lower, i, label) {
                label_hit = Some(label.len());
                break;
            }
        }
        if let Some(label_len) = label_hit {
            let end = i + label_len;
            out.extend(chars[i..end].iter());
            let mut j = end;
            while j < n && (chars[j].is_whitespace() || chars[j] == '\'' || chars[j] == '"') {
                out.push(chars[j]);
                j += 1;
            }
            if j < n && (chars[j] == ':' || chars[j] == '=') {
                out.push(chars[j]);
                j += 1;
                while j < n && chars[j].is_whitespace() {
                    out.push(chars[j]);
                    j += 1;
                }
                let quoted = j < n && chars[j] == '"';
                if quoted {
                    out.push(chars[j]);
                    j += 1;
                }
                let start = j;
                while j < n
                    && !chars[j].is_whitespace()
                    && chars[j] != '"'
                    && chars[j] != ','
                    && chars[j] != '}'
                    && chars[j] != ']'
                {
                    j += 1;
                }
                if j > start {
                    out.push_str(REDACTED);
                }
                i = j;
                continue;
            }
            // Not an assignment (e.g. the word `token` inside a
            // sentence): the label itself is not a secret — everything
            // already emitted stands and scanning resumes past it.
            i = j;
            continue;
        }
        // (e) a ≥32-char token run adjacent to a key-ish label.
        if is_token_char(chars[i]) {
            let mut j = i;
            while j < n && is_token_char(chars[j]) {
                j += 1;
            }
            if j - i >= 32 && beside_keyish_label(&out) {
                out.push_str(REDACTED);
                i = j;
                continue;
            }
            // An ordinary run: copy one char and let the loop re-scan
            // (the next position may open a scheme or a label).
            out.push(chars[i]);
            i += 1;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// The ONE writer of the `upstream_body` key (ADR-060 D2's
/// "impossible to bypass" rule): attach a slice produced by [`capture`]
/// to a failure's `details`. Called on the failure record's own copy —
/// the client-facing body keeps its shape untouched.
pub(crate) fn attach(failure: &mut crate::forward::ForwardFailure, slice: Value) {
    let details = failure.details.get_or_insert_with(|| json!({}));
    if let Some(obj) = details.as_object_mut() {
        obj.insert("upstream_body".to_string(), slice);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cred() -> Vec<&'static str> {
        vec!["sk-live-SECRETVALUE123"]
    }

    /// The allowlist: only the four fields (and `error.{…}` one level
    /// down) survive; a body that echoes the request contributes
    /// nothing.
    #[test]
    fn allowlist_drops_every_other_key() {
        let body = br#"{"error":{"message":"nope","type":"invalid_request_error"},"message":"Unknown parameter: foo","type":"invalid_request_error","code":400,"param":"messages[2].content","request_id":"req_abc","model":"gpt-x","echoed_input":"a whole user message"}"#;
        let v = capture(body, &[]).expect("fields survive");
        assert_eq!(v["shape"], "json");
        assert_eq!(v["message"], "Unknown parameter: foo");
        assert_eq!(v["type"], "invalid_request_error");
        assert_eq!(v["code"], "400");
        assert_eq!(v["param"], "messages[2].content");
        assert_eq!(v["error"]["message"], "nope");
        assert_eq!(v["error"]["type"], "invalid_request_error");
        for banned in ["request_id", "model", "echoed_input"] {
            assert!(v.get(banned).is_none(), "{banned} must be dropped");
        }
    }

    /// Present iff something survived: a JSON object with no allowlisted
    /// member is absent, and so is an empty body.
    #[test]
    fn absent_means_nothing_survived() {
        assert!(capture(b"", &[]).is_none(), "no body at all");
        assert!(
            capture(br#"{"request_id":"req_1","model":"m"}"#, &[]).is_none(),
            "parsable object with nothing retained"
        );
    }

    /// An unparsable body is the text shape: the byte count, no bytes.
    #[test]
    fn unparsable_body_carries_the_count_and_no_bytes() {
        let v = capture(b"Internal Server Error: boom", &[]).expect("text shape");
        assert_eq!(v["shape"], "text");
        assert_eq!(v["bytes"], 27);
        assert_eq!(v.as_object().unwrap().len(), 2, "nothing else");
    }

    /// A body larger than the capture bound is judged on its first 1024
    /// bytes only — and since a truncated JSON prefix does not parse,
    /// the text shape is what a reader gets: the count of the bytes
    /// held, never the body.
    #[test]
    fn the_capture_bound_is_1024_bytes() {
        let mut big = br#"{"message":""#.to_vec();
        big.resize(5000, b'a');
        let v = capture(&big, &[]).expect("the text shape is a record");
        assert_eq!(v["shape"], "text");
        assert_eq!(v["bytes"], 1024, "the count is of the capture, never more");
        assert_eq!(v.as_object().unwrap().len(), 2, "no bytes, nothing else");
    }

    /// The per-field bounds: message ≤ 256, the others ≤ 64, cut on a
    /// UTF-8 boundary, with `truncated: true`.
    #[test]
    fn per_field_bounds_cut_on_a_char_boundary() {
        let long_msg = "ü".repeat(300); // 600 bytes, 2 per char
        let body = format!(r#"{{"message":"{long_msg}","code":"{}"}}"#, "c".repeat(80));
        let v = capture(body.as_bytes(), &[]).expect("parses");
        assert_eq!(v["truncated"], true);
        let m = v["message"].as_str().unwrap();
        assert!(m.len() <= 256 && m.chars().all(|c| c == 'ü'));
        let c = v["code"].as_str().unwrap();
        assert!(c.len() <= 64, "the other fields bound to 64");
    }

    /// Redaction (a): the process's own configured credential value,
    /// exact match.
    #[test]
    fn a_configured_credential_is_redacted_exactly() {
        let body = format!(r#"{{"message":"key sk-live-SECRETVALUE123 was rejected"}}"#);
        let v = capture(body.as_bytes(), &cred()).expect("parses");
        assert_eq!(v["message"], "key [redacted] was rejected");
        assert!(!v["message"].as_str().unwrap().contains("SECRETVALUE"));
    }

    /// Redaction (b)+(c): auth schemes and key shapes. The scheme word
    /// stays (it says what was redacted); the credential goes.
    #[test]
    fn schemes_and_key_shapes_are_redacted() {
        let cases = [
            (
                "Authorization: Bearer abc123DEFghi",
                "Authorization: Bearer [redacted]",
            ),
            (
                "retry with basic dXNlcjpwYXNz",
                "retry with basic [redacted]",
            ),
            (
                "invalid sk-proj-9f8e7d6c5b4a3210 token",
                "invalid [redacted] token",
            ),
            ("bad pk-9f8e7d6c5b4a3210", "bad [redacted]"),
        ];
        for (input, want) in cases {
            let got = redact(input, &[]);
            assert_eq!(got, want, "input: {input}");
            assert!(!got.contains("abc123"), "the credential is gone");
        }
    }

    /// Redaction (d): a key-ish assignment, quoted or not.
    #[test]
    fn key_assignments_are_redacted() {
        assert_eq!(
            redact(r#"api_key = "super-secret-value-123""#, &[]),
            r#"api_key = "[redacted]""#
        );
        assert_eq!(
            redact("the api-key:abcdef123456 is stale", &[]),
            "the api-key:[redacted] is stale"
        );
        assert_eq!(
            redact("token: ghp_16C7e42F292c6912E7710c838347Ae178B4a", &[]),
            "token: [redacted]"
        );
        assert_eq!(redact("password=hunter2!", &[]), "password=[redacted]");
    }

    /// Redaction (e): a ≥32-char run beside a key-ish label, while the
    /// same run without a label survives (the negative arm — the rule
    /// is adjacency, not length alone).
    #[test]
    fn long_runs_are_redacted_only_beside_a_keyish_label() {
        let run = "AbCdEf012345678901234567890123456789"; // 36 chars
        assert_eq!(redact(&format!("key {run}"), &[]), "key [redacted]");
        assert_eq!(redact(&format!("id {run}"), &[]), format!("id {run}"));
    }

    /// The word `token` inside an ordinary sentence is a word, not a
    /// label-with-assignment: the rule does not eat prose.
    #[test]
    fn a_label_without_an_assignment_is_prose() {
        assert_eq!(
            redact("your token quota is fine", &[]),
            "your token quota is fine"
        );
    }
}
