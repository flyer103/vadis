//! The transform-mode domain types (ADR-019, DESIGN §12.12, spec §2.1/§6).
//!
//! Split on purpose, so the half that decides is testable without a body and
//! the half that edits is auditable by span comparison:
//!
//! - [`TransformMode`] — the request fact resolved from
//!   `X-Router-Transform` **by `vadis-proxy`** (vadis-core never reads a
//!   header, §12.5/§12.11's split). `Passthrough` is the default; absence of
//!   the header is a byte-level guarantee (ADR-019 item 1).
//! - [`NodePath`] / [`PayloadEdit`] — a content-level, content-addressed
//!   plan: which payload node, which rule, which new text. Spans are
//!   resolved later, against the bytes being edited, by
//!   [`crate::body::RawBody::apply_edits`].
//! - [`payload_nodes`] — the payload locator: the tool/environment payload
//!   nodes of a body, per wire shape, with the tool name each rule's
//!   `match_tool` selects on. Spans only; never a reserialize.
//! - [`TransformEngine`] — the rule engine seam (implemented by
//!   `vadis-plugins::builtin transform_rules`): **text in, text out, per
//!   payload node**. The sufficient rule for I1 (DESIGN §12.12): an edit may
//!   depend only on the node it edits plus the stable rule set.
//!
//! I1's sufficient rule is enforced by shape, not by hope: nothing in this
//! module or the engine trait can observe a clock, a turn index, a session
//! history or an RNG — the inputs are (node text, node context, rule set).

use serde::Serialize;

use crate::body::RawBody;
use crate::config::WireApi;

/// The mode in effect for one request's outbound body (ADR-019 §2, spec
/// §2.1/§6). Always present on the trace record; `passthrough` on every
/// request that did not ask (and on every request refused before the
/// transform chain ran — no step was even planned, so nothing is claimed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TransformMode {
    /// The byte path: the client's bytes + exactly the two mutations of
    /// spec §2, nothing else — even when matching rules are configured
    /// (invariant I3).
    Passthrough,
    /// The declared-edit path: the client's bytes + an ordered list of
    /// value-span edits over tool/environment payloads + the same two
    /// mutations (ADR-019 §3).
    Transform,
}

impl TransformMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passthrough => "passthrough",
            Self::Transform => "transform",
        }
    }
}

/// One segment of a node address: an object key or an array index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathSeg {
    Key(String),
    Index(u32),
}

/// A node address within the request body, e.g. `input[7].output` or
/// `messages[3].content` (DESIGN §12.12). Never a byte offset — the scan is
/// what resolves it, against the bytes being edited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodePath(pub Vec<PathSeg>);

impl NodePath {
    /// The display form used in the trace's `edited_paths[].path` (spec §6):
    /// `input[7].output`. Keys are printed bare (the payload paths this
    /// module addresses never need escaping).
    pub fn display(&self) -> String {
        let mut out = String::new();
        for (i, seg) in self.0.iter().enumerate() {
            match seg {
                PathSeg::Key(k) => {
                    if i > 0 {
                        out.push('.');
                    }
                    out.push_str(k);
                }
                PathSeg::Index(n) => {
                    out.push('[');
                    out.push_str(&n.to_string());
                    out.push(']');
                }
            }
        }
        out
    }
}

/// One planned content edit (the plan is content-addressed; the bytes are
/// resolved at application time).
#[derive(Debug, Clone)]
pub struct PayloadEdit {
    pub path: NodePath,
    /// The rule id that produced the edit (the trace's `plugin`).
    pub rule: String,
    /// The node's new text (decoded; the splicer re-encodes it as a JSON
    /// string value).
    pub new_text: String,
    /// The payload text's byte length before the step (spec §6: the payload
    /// text's length, not the encoded span's).
    pub bytes_in: usize,
    /// The payload text's byte length after the step.
    pub bytes_out: usize,
}

/// The node context a rule selects on (ADR-019 §3: "selected by declaration,
/// never by guessing at content"). `tool` is the tool name from the wire's
/// own pairing (a `tool_call_id` / `call_id` resolved against the
/// conversation's earlier call items — a pure function of the bytes); `kinds`
/// is the payload category, GAP-Q18's settlement for this slice (see
/// [`TOOL_KINDS`]).
pub struct PayloadCtx<'a> {
    pub tool: Option<&'a str>,
    pub kinds: &'a [&'a str],
}

/// The engine's answer for one node: the new text plus what the ledger needs.
/// `rule` is the **rule id that fired** for this node (the trace's `plugin`
/// is "the rule id", spec §6 — one engine carries many rules, and the
/// ledger is per step, i.e. per rule). `cache_impact` is the stable word
/// `neutral` / `risky` / `broken` (§12.3's vocabulary); a per-node trim is
/// `neutral` by construction (it depends only on the node it edits, which
/// is I2's sufficient rule).
pub struct TransformOutcome {
    /// The rule id that produced this edit (the ledger entry's `plugin`).
    pub rule: String,
    pub new_text: String,
    pub cache_impact: &'static str,
    /// The tee marker's identity (sha16 of the original payload) when the
    /// rule appended one; `None` otherwise. Storage/retrieval stay
    /// unimplemented in v0.1 (spec §4.4).
    pub tee_id: Option<String>,
}

/// The rule-engine seam (DESIGN §12.12's superseded `Transform` shape):
/// text in, text out, per payload node. `None` = leave this node alone.
/// Implementations must be pure functions of (text, ctx, stable rule set) —
/// AGENTS hard constraint 2 via the per-node contract.
pub trait TransformEngine: Send + Sync {
    fn id(&self) -> &'static str;
    fn apply_node(&self, ctx: &PayloadCtx<'_>, text: &str) -> Option<TransformOutcome>;
}

/// GAP-Q18's settlement for this slice (recorded with the card): a rule's
/// `match_kind` is a payload category declaration with **no wire field** in
/// v0.1, so the category is derived from the tool name via this declared,
/// content-free table — the engine never sniffs the payload text to guess a
/// kind. Tools absent from the table carry no kind: rules selecting on
/// `match_kind` alone do not fire for them (`match_tool` remains the tier-1
/// selection path, which is what the contract depends on).
pub const TOOL_KINDS: &[(&str, &[&str])] = &[
    // shell-family: command output — a log, and text.
    ("^(Bash|bash|shell|run_terminal_cmd)$", &["log", "text"]),
    // search-family: hit lines — text.
    ("^(Grep|grep|rg|ripgrep|search)$", &["text"]),
    // patch-family: diff payloads.
    ("^(Diff|diff|apply_patch|Apply_patch)$", &["diff"]),
];

/// The kinds declared for a tool name (empty when no table row matches).
/// Regex matching is the caller's (vadis-plugins') job; this table is the
/// single declaration both sides share.
pub fn kinds_for_tool(tool: &str) -> &'static [&'static str] {
    for (pat, kinds) in TOOL_KINDS {
        // The table's patterns are exact-name alternations; compare without
        // pulling regex into vadis-core (its allowlist has no regex row).
        // `^(A|B|C)$` → the set {A, B, C}.
        let inner = pat
            .strip_prefix("^(")
            .and_then(|p| p.strip_suffix(")$"))
            .unwrap_or(pat);
        if inner.split('|').any(|name| name == tool) {
            return kinds;
        }
    }
    &[]
}

/// The inferred-token estimate (GAP-Q14: the dependency allowlist has no
/// tokenizer; spec §7 — a local byte-length attribution is `inferred` and
/// says so, and it never enters a gate). bytes/4, floor — one convention,
/// one home.
pub fn estimate_tokens(bytes: usize) -> i64 {
    (bytes / 4) as i64
}

/// One located payload node: its address, its decoded text, and the tool
/// name the wire's own pairing resolved (None when the shape carries no
/// name for it).
pub struct PayloadNode {
    pub path: NodePath,
    pub text: String,
    pub tool: Option<String>,
}

/// The payload locator (DESIGN §12.12): the tool/environment payload nodes
/// of a body, per wire shape, by span scan. Payloads only — a user or
/// assistant message, the system instruction, the tool schemas and every
/// structural member are never returned, so the mode's "payloads only" rule
/// (ADR-019 §3) holds by construction:
///
/// - chat: `messages[n].content` where `role == "tool"`; the tool name is
///   the `function.name` of the **earlier** assistant `tool_calls` entry
///   whose `id` equals this message's `tool_call_id` (a pure function of
///   the bytes — pairing walks backwards through what is present, never
///   through session state).
/// - responses: `input[n].output` where `type == "function_call_output"`; the
///   name from the earlier `function_call` item with the same `call_id`.
/// - anthropic: `messages[n].content[m].content` where the block's
///   `type == "tool_result"`; the name is the block's own `name` member.
///
/// Non-string payload values (an array content) are not text nodes and are
/// skipped. A body whose top level is not a scannable object yields no nodes
/// (the forwarding path answers its own 400 for that class).
pub fn payload_nodes(body: &RawBody, proto: WireApi) -> Vec<PayloadNode> {
    let spans = match body.top_level_member_spans() {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let b = body.as_bytes();
    let mut out = Vec::new();
    match proto {
        WireApi::Chat => {
            let Some(&(_, s, e)) = spans.iter().find(|(k, _, _)| k == "messages") else {
                return out;
            };
            // (element span, role, tool_call_id, content span) per message.
            let mut calls: Vec<(String, String)> = Vec::new(); // (id, name)
            for (n, (el_s, el_e)) in array_elements(b, s, e).into_iter().enumerate() {
                let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b[el_s..el_e]) else {
                    continue;
                };
                // Pairing bookkeeping first (assistant tool_calls precede the
                // role=tool messages that reference them).
                if v.get("role").and_then(|r| r.as_str()) == Some("assistant") {
                    if let Some(tcs) = v.get("tool_calls").and_then(|t| t.as_array()) {
                        for tc in tcs {
                            let id = tc.get("id").and_then(|i| i.as_str());
                            let name = tc
                                .get("function")
                                .and_then(|f| f.get("name"))
                                .and_then(|n| n.as_str());
                            if let (Some(id), Some(name)) = (id, name) {
                                calls.push((id.to_string(), name.to_string()));
                            }
                        }
                    }
                }
                if v.get("role").and_then(|r| r.as_str()) != Some("tool") {
                    continue;
                }
                let Some(content) = v.get("content").and_then(|c| c.as_str()) else {
                    continue;
                };
                let tool = v
                    .get("tool_call_id")
                    .and_then(|i| i.as_str())
                    .and_then(|id| {
                        calls
                            .iter()
                            .find(|(cid, _)| cid == id)
                            .map(|(_, name)| name.clone())
                    });
                out.push(PayloadNode {
                    path: NodePath(vec![
                        PathSeg::Key("messages".into()),
                        PathSeg::Index(n as u32),
                        PathSeg::Key("content".into()),
                    ]),
                    text: content.to_string(),
                    tool,
                });
            }
        }
        WireApi::Responses => {
            let Some(&(_, s, e)) = spans.iter().find(|(k, _, _)| k == "input") else {
                return out;
            };
            let mut calls: Vec<(String, String)> = Vec::new(); // (call_id, name)
            for (n, (el_s, el_e)) in array_elements(b, s, e).into_iter().enumerate() {
                let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b[el_s..el_e]) else {
                    continue;
                };
                if v.get("type").and_then(|t| t.as_str()) == Some("function_call") {
                    let id = v.get("call_id").and_then(|i| i.as_str());
                    let name = v.get("name").and_then(|n| n.as_str());
                    if let (Some(id), Some(name)) = (id, name) {
                        calls.push((id.to_string(), name.to_string()));
                    }
                }
                if v.get("type").and_then(|t| t.as_str()) != Some("function_call_output") {
                    continue;
                }
                let Some(output) = v.get("output").and_then(|o| o.as_str()) else {
                    continue;
                };
                let tool = v.get("call_id").and_then(|i| i.as_str()).and_then(|id| {
                    calls
                        .iter()
                        .find(|(cid, _)| cid == id)
                        .map(|(_, name)| name.clone())
                });
                out.push(PayloadNode {
                    path: NodePath(vec![
                        PathSeg::Key("input".into()),
                        PathSeg::Index(n as u32),
                        PathSeg::Key("output".into()),
                    ]),
                    text: output.to_string(),
                    tool,
                });
            }
        }
        WireApi::Anthropic => {
            let Some(&(_, s, e)) = spans.iter().find(|(k, _, _)| k == "messages") else {
                return out;
            };
            for (n, (el_s, el_e)) in array_elements(b, s, e).into_iter().enumerate() {
                let Ok(v) = serde_json::from_slice::<serde_json::Value>(&b[el_s..el_e]) else {
                    continue;
                };
                let Some(blocks) = v.get("content").and_then(|c| c.as_array()) else {
                    continue;
                };
                for (m, block) in blocks.iter().enumerate() {
                    if block.get("type").and_then(|t| t.as_str()) != Some("tool_result") {
                        continue;
                    }
                    let Some(content) = block.get("content").and_then(|c| c.as_str()) else {
                        continue;
                    };
                    out.push(PayloadNode {
                        path: NodePath(vec![
                            PathSeg::Key("messages".into()),
                            PathSeg::Index(n as u32),
                            PathSeg::Key("content".into()),
                            PathSeg::Index(m as u32),
                            PathSeg::Key("content".into()),
                        ]),
                        text: content.to_string(),
                        tool: block
                            .get("name")
                            .and_then(|n| n.as_str())
                            .map(str::to_string),
                    });
                }
            }
        }
    }
    out
}

/// The byte spans `(start, end)` of one JSON array's elements, in order.
/// Bracket/string tracking only — the same discipline as the prefix
/// extractor's element scan, never a reserialize.
fn array_elements(b: &[u8], val_start: usize, val_end: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = val_start;
    if i >= b.len() || b[i] != b'[' {
        return out;
    }
    i += 1;
    loop {
        while i < val_end && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i >= val_end || b[i] == b']' {
            break;
        }
        let el_start = i;
        i = crate::prefix::scan_element_end(b, i, val_end);
        out.push((el_start, i));
        while i < val_end && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i < val_end && b[i] == b',' {
            i += 1;
        } else {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::RawBody;
    use std::borrow::Cow;

    fn edit(path: &[PathSeg], new_text: &str) -> PayloadEdit {
        PayloadEdit {
            path: NodePath(path.to_vec()),
            rule: "test".to_string(),
            new_text: new_text.to_string(),
            bytes_in: 0,
            bytes_out: new_text.len(),
        }
    }

    fn segs(s: &[&str]) -> Vec<PathSeg> {
        s.iter().map(|k| PathSeg::Key(k.to_string())).collect()
    }

    // ---- apply_edits: the adversarial matrix (DESIGN §12.12 order ①) ----

    #[test]
    fn empty_edits_borrow_the_input() {
        let raw = RawBody::new(br#"{"model":"a"}"#.to_vec());
        match raw.apply_edits(&[]) {
            Ok(Cow::Borrowed(b)) => assert_eq!(b, br#"{"model":"a"}"#),
            _ => panic!("empty edits must borrow"),
        }
    }

    // 1. Structural chars inside a sibling string are not mistaken for
    //    structure; every byte outside the edited span survives verbatim.
    #[test]
    fn sibling_structural_chars_survive_verbatim() {
        let raw = RawBody::new(
            r#"{"model":"p/m","messages":[{"role":"tool","content":"a{b}c:d,e"}],"zz":"{[}]"}"#
                .as_bytes()
                .to_vec(),
        );
        let out = raw
            .apply_edits(&[edit(
                &[
                    PathSeg::Key("messages".into()),
                    PathSeg::Index(0),
                    PathSeg::Key("content".into()),
                ],
                "trimmed",
            )])
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out),
            r#"{"model":"p/m","messages":[{"role":"tool","content":"trimmed"}],"zz":"{[}]"}"#
        );
    }

    // 2. Escapes: the new text is re-encoded (a literal newline becomes
    //    \n) and escaped bytes in untouched spans survive byte for byte.
    #[test]
    fn new_text_is_reencoded_and_escapes_survive() {
        let raw = RawBody::new(r#"{"k":"keep \"q\" \\","nested":{"s":"old"}}"#.as_bytes().to_vec());
        let out = raw
            .apply_edits(&[edit(
                &[PathSeg::Key("nested".into()), PathSeg::Key("s".into())],
                "line1\nline2\t\"x\"",
            )])
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out),
            r#"{"k":"keep \"q\" \\","nested":{"s":"line1\nline2\t\"x\""}}"#
        );
    }

    // 3. Multi-byte UTF-8: bytes >= 0x80 never collide with structural
    //    ASCII, so CJK content splices without corruption.
    #[test]
    fn multibyte_utf8_splices_cleanly() {
        let raw = RawBody::new(
            r#"{"messages":[{"role":"tool","content":"日志输出第一行"}]}"#
                .as_bytes()
                .to_vec(),
        );
        let out = raw
            .apply_edits(&[edit(
                &[
                    PathSeg::Key("messages".into()),
                    PathSeg::Index(0),
                    PathSeg::Key("content".into()),
                ],
                "截断后",
            )])
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out),
            r#"{"messages":[{"role":"tool","content":"截断后"}]}"#
        );
    }

    // 4. A path resolving to a non-string value is an error, never an
    //    invention.
    #[test]
    fn non_string_target_is_an_error() {
        let raw = RawBody::new(br#"{"n":42}"#.to_vec());
        assert!(raw.apply_edits(&[edit(&segs(&["n"]), "x")]).is_err());
    }

    // 5. An absent path is an error.
    #[test]
    fn absent_path_is_an_error() {
        let raw = RawBody::new(br#"{"a":"b"}"#.to_vec());
        assert!(raw.apply_edits(&[edit(&segs(&["missing"]), "x")]).is_err());
        // And an out-of-range index.
        let raw2 = RawBody::new(br#"{"a":["x"]}"#.to_vec());
        assert!(raw2
            .apply_edits(&[edit(&[PathSeg::Key("a".into()), PathSeg::Index(3)], "x")])
            .is_err());
    }

    // 6. Two edits resolving to the same span are an error (a plan that
    //    cannot be described as disjoint spans).
    #[test]
    fn duplicate_span_is_an_error() {
        let raw = RawBody::new(br#"{"a":"x"}"#.to_vec());
        let e = edit(&segs(&["a"]), "y");
        assert!(raw.apply_edits(&[e.clone(), e]).is_err());
    }

    // 7. Multiple edits in *unsorted* plan order still land correctly
    //    (ascending-span splicing is the applier's own discipline).
    #[test]
    fn unsorted_multi_edit_plan_applies_by_span_order() {
        let raw = RawBody::new(br#"{"a":"one","b":"two","c":"three"}"#.to_vec());
        let edits = vec![
            edit(&segs(&["c"]), "C"),
            edit(&segs(&["a"]), "A"),
            edit(&segs(&["b"]), "B"),
        ];
        let out = raw.apply_edits(&edits).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out),
            r#"{"a":"A","b":"B","c":"C"}"#
        );
    }

    // 8. I1 shape at the unit level: identical (bytes, edits) → identical
    //    output, across repeated calls.
    #[test]
    fn apply_edits_is_deterministic() {
        let raw = RawBody::new(br#"{"x":{"y":["a","b","c"]}}"#.to_vec());
        let edits = vec![edit(
            &[
                PathSeg::Key("x".into()),
                PathSeg::Key("y".into()),
                PathSeg::Index(1),
            ],
            "B",
        )];
        let o1 = raw.apply_edits(&edits).unwrap();
        let o2 = raw.apply_edits(&edits).unwrap();
        assert_eq!(o1, o2);
        assert_eq!(String::from_utf8_lossy(&o1), r#"{"x":{"y":["a","B","c"]}}"#);
    }

    // 9. Whitespace between members is preserved byte for byte (only the
    //    addressed value spans change).
    #[test]
    fn whitespace_outside_spans_is_preserved() {
        let raw = RawBody::new(b"{\n  \"a\" : \"x\" ,\n  \"b\":\"y\"\n}".to_vec());
        let out = raw.apply_edits(&[edit(&segs(&["a"]), "X")]).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out),
            "{\n  \"a\" : \"X\" ,\n  \"b\":\"y\"\n}"
        );
    }

    // ---- NodePath display ----

    #[test]
    fn node_path_display_forms() {
        let p = NodePath(vec![
            PathSeg::Key("input".into()),
            PathSeg::Index(7),
            PathSeg::Key("output".into()),
        ]);
        assert_eq!(p.display(), "input[7].output");
        let leading = NodePath(vec![PathSeg::Index(0)]);
        assert_eq!(leading.display(), "[0]");
    }

    // ---- kinds_for_tool (GAP-Q18's declared table) ----

    #[test]
    fn kinds_table_is_content_free() {
        assert_eq!(kinds_for_tool("Bash"), &["log", "text"] as &[&str]);
        assert_eq!(kinds_for_tool("Grep"), &["text"] as &[&str]);
        assert_eq!(kinds_for_tool("diff"), &["diff"] as &[&str]);
        // Unknown tools carry no kind: a match_kind-only rule does not
        // fire for them (match_tool is the tier-1 selection path).
        assert!(kinds_for_tool("custom_tool").is_empty());
    }

    // ---- payload_nodes: the three wire shapes ----

    #[test]
    fn chat_payload_nodes_pair_tool_calls() {
        let body = br#"{"model":"m","messages":[
            {"role":"user","content":"hi"},
            {"role":"assistant","tool_calls":[{"id":"c1","function":{"name":"Bash"}}]},
            {"role":"tool","tool_call_id":"c1","content":"line1\nline2"},
            {"role":"assistant","content":"done"}
        ]}"#;
        let raw = RawBody::new(body.to_vec());
        let nodes = payload_nodes(&raw, WireApi::Chat);
        assert_eq!(nodes.len(), 1, "only the role=tool message is a payload");
        assert_eq!(nodes[0].path.display(), "messages[2].content");
        assert_eq!(nodes[0].text, "line1\nline2");
        assert_eq!(nodes[0].tool.as_deref(), Some("Bash"));
    }

    #[test]
    fn responses_payload_nodes_pair_call_id() {
        let body = br#"{"model":"m","input":[
            {"type":"message","role":"user","content":"hi"},
            {"type":"function_call","call_id":"c7","name":"Grep","arguments":"{}"},
            {"type":"function_call_output","call_id":"c7","output":"hit1\nhit2"},
            {"type":"message","role":"assistant","content":"done"}
        ]}"#;
        let raw = RawBody::new(body.to_vec());
        let nodes = payload_nodes(&raw, WireApi::Responses);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].path.display(), "input[2].output");
        assert_eq!(nodes[0].tool.as_deref(), Some("Grep"));
    }

    #[test]
    fn anthropic_payload_nodes_read_block_name() {
        let body = br#"{"model":"m","messages":[
            {"role":"user","content":[{"type":"text","text":"hi"}]},
            {"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"diff","input":{}}]},
            {"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","name":"diff","content":"--- a\n+++ b"}]}
        ]}"#;
        let raw = RawBody::new(body.to_vec());
        let nodes = payload_nodes(&raw, WireApi::Anthropic);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].path.display(), "messages[2].content[0].content");
        assert_eq!(nodes[0].tool.as_deref(), Some("diff"));
    }

    #[test]
    fn user_and_assistant_messages_are_never_payloads() {
        // The mode's "payloads only" rule holds by construction: a user
        // message and an assistant message yield no nodes on any shape.
        for (proto, body) in [
            (
                WireApi::Chat,
                &br#"{"messages":[{"role":"user","content":"u"},{"role":"assistant","content":"a"}]}"#[..],
            ),
            (
                WireApi::Responses,
                &br#"{"input":[{"type":"message","role":"user","content":"u"},{"type":"message","role":"assistant","content":"a"}]}"#[..],
            ),
            (
                WireApi::Anthropic,
                &br#"{"messages":[{"role":"user","content":"u"},{"role":"assistant","content":"a"}]}"#[..],
            ),
        ] {
            let raw = RawBody::new(body.to_vec());
            assert!(
                payload_nodes(&raw, proto).is_empty(),
                "{proto:?}: no payload nodes from user/assistant messages"
            );
        }
    }

    /// A brace/bracket inside a JSON string must not open a nesting level:
    /// the element scanner's string state is pushed AND the opening quote
    /// is stepped past in the same move. Found by CONF-63's I2 fixture (a
    /// grep payload containing `fn unrelated() {`): before the fix the
    /// quote was re-read with the string state on top and popped it
    /// immediately, so the payload's `{` corrupted the element span and
    /// the tool message silently vanished from the locator's output.
    #[test]
    fn structural_chars_inside_strings_do_not_open_nesting() {
        let body = br#"{"messages":[{"role":"tool","content":"a{b}c [d] e"},{"role":"user","content":"u"}],"x":[1,2]}"#;
        let raw = RawBody::new(body.to_vec());
        let spans = raw.top_level_member_spans().expect("scannable");
        let (_, s, e) = spans.iter().find(|(k, _, _)| k == "messages").unwrap();
        // Two message elements: the `{`/`[` inside the first element's
        // string must not swallow the `]` that closes the messages array.
        let els = array_elements(body, *s, *e);
        assert_eq!(els.len(), 2, "both array elements are found");
        assert_eq!(
            &body[els[0].0..els[0].1],
            br#"{"role":"tool","content":"a{b}c [d] e"}"#
        );
        assert_eq!(
            &body[els[1].0..els[1].1],
            br#"{"role":"user","content":"u"}"#,
            "the second element is not swallowed by the first's brace"
        );
    }

    #[test]
    fn roundtrip_locate_then_edit_reaches_the_same_node() {
        // The locator's addresses are the applier's addresses: what
        // payload_nodes found is exactly what apply_edits splices.
        let body = br#"{"messages":[
            {"role":"assistant","tool_calls":[{"id":"c1","function":{"name":"Bash"}}]},
            {"role":"tool","tool_call_id":"c1","content":"noise line\nreal line"}
        ]}"#;
        let raw = RawBody::new(body.to_vec());
        let nodes = payload_nodes(&raw, WireApi::Chat);
        let edits: Vec<PayloadEdit> = nodes
            .into_iter()
            .map(|n| PayloadEdit {
                path: n.path,
                rule: "bash-log-noise".into(),
                new_text: "real line".into(),
                bytes_in: n.text.len(),
                bytes_out: "real line".len(),
            })
            .collect();
        let out = raw.apply_edits(&edits).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out),
            "{\"messages\":[\n            {\"role\":\"assistant\",\"tool_calls\":[{\"id\":\"c1\",\"function\":{\"name\":\"Bash\"}}]},\n            {\"role\":\"tool\",\"tool_call_id\":\"c1\",\"content\":\"real line\"}\n        ]}"
        );
    }
}
