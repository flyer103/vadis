//! Prefix blocks and the prefix hash (DESIGN §12.10.6, spec §6).
//!
//! Both the trace's `prefix_blocks[]` and the event's `body_hash` are
//! computed at exactly one place: the encoder's output — the byte-final
//! outbound body. Extraction is a span scan over that body (never a
//! parse→reserialize); `body_sha16` is the single hash helper, so two
//! implementations cannot drift because there is only one.
//!
//! Block `tokens` (GAP-Q14): the dependency allowlist has no tokenizer, so
//! a block's token count is a proportional attribution of the measured
//! `usage.input_total` over the prefix region by block byte length — an
//! **`inferred`** figure everywhere it is reported (spec §7).

use sha2::{Digest, Sha256};

use crate::body::RawBody;
use crate::cost::Usage;

/// First 16 hex chars of sha256 — the one hash convention (spec §6 block
/// hash, `events.body_hash`, and any future digest).
pub fn body_sha16(bytes: &[u8]) -> String {
    let d = Sha256::digest(bytes);
    let mut out = String::with_capacity(16);
    for b in &d[..8] {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// The structural block kinds of the prefix domain (spec §6):
/// `messages` / `input` / `tools` members plus the system-instruction
/// position (`system` for the anthropic wire shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockKind {
    Message,
    Tool,
    InputItem,
    System,
}

impl BlockKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Message => "message",
            Self::Tool => "tool",
            Self::InputItem => "input_item",
            Self::System => "system",
        }
    }
}

/// One prefix block: a structural unit's span in the outbound body, its
/// hash, and its (inferred) token share. `tokens` is 0 until
/// [`attribute_tokens`] runs — hashes are the fidelity signal; tokens are
/// money-side metadata.
#[derive(Debug, Clone)]
pub struct PrefixBlock {
    pub index: u32,
    pub kind: BlockKind,
    pub hash: String,
    pub byte_len: usize,
    pub tokens: u64,
}

/// Extracts the prefix blocks from an outbound body by span scan.
///
/// The domain (spec §6): the `messages` or `input` array (one block per
/// element) and the `tools` array (one block per definition), plus the
/// top-level `system` member (the anthropic system-instruction position)
/// as the **first** block when present. Everything else is not prefix.
/// Vadis-owned top-level keys are already gone from the outbound body, so
/// deleting them cannot change a block hash (CONF-10's claim).
///
/// **Enumeration order = the provider's effective prompt (template)
/// order** (2026-09-20 user decision, Plan A): system-instruction
/// position → `tools` → `messages` / `input` items — never body byte
/// order. The measured codex body serializes `input` before `tools` while
/// the provider template places tools before the conversation, so under
/// byte order a client's tail append (in template order) registered as a
/// mid-sequence insertion and continuity under-reported a ~4× gap against
/// the upstream's own verified hit rate. CONF-31 pins this order.
pub fn extract_prefix_blocks(
    body: &RawBody,
) -> Result<Vec<PrefixBlock>, crate::body::RawEditError> {
    let spans = body.top_level_member_spans()?;
    let b = body.as_bytes();
    let mut blocks = Vec::new();

    let mut push = |kind: BlockKind, val_start: usize, val_end: usize| {
        let hash = body_sha16(&b[val_start..val_end]);
        blocks.push(PrefixBlock {
            index: blocks.len() as u32,
            kind,
            hash,
            byte_len: val_end - val_start,
            tokens: 0,
        });
    };

    // The system-instruction position sits at the very front of the prefix
    // (spec §2's mapping rule: keep the position stable).
    if let Some(&(_, s, e)) = spans.iter().find(|(k, _, _)| k == "system") {
        push(BlockKind::System, s, e);
    }
    // Then the template order: tools before the conversation members.
    // `messages` and `input` never coexist on one wire shape; visiting
    // `messages` first keeps a single deterministic order for both.
    for wanted in ["tools", "messages", "input"] {
        for (k, s, e) in &spans {
            if k.as_str() != wanted {
                continue;
            }
            let kind = match wanted {
                "tools" => BlockKind::Tool,
                "messages" => BlockKind::Message,
                _ => BlockKind::InputItem,
            };
            scan_array_elements(b, *s, *e, kind, &mut blocks);
        }
    }
    Ok(blocks)
}

/// Scans one array's elements from its top-level value span, one block per
/// element. Uses serde_json's **value** view only to locate element
/// boundaries via spans computed by the same scanner discipline (element
/// spans are determined by bracket/string tracking, never by reserializing).
fn scan_array_elements(
    b: &[u8],
    val_start: usize,
    val_end: usize,
    kind: BlockKind,
    out: &mut Vec<PrefixBlock>,
) {
    // The span is the array literal `[...]`; walk its elements.
    let mut i = val_start;
    if i >= b.len() || b[i] != b'[' {
        return;
    }
    i += 1;
    loop {
        // skip whitespace
        while i < val_end && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i >= val_end || b[i] == b']' {
            break;
        }
        let el_start = i;
        i = scan_element_end(b, i, val_end);
        let el_end = i;
        let hash = body_sha16(&b[el_start..el_end]);
        out.push(PrefixBlock {
            index: out.len() as u32,
            kind,
            hash,
            byte_len: el_end - el_start,
            tokens: 0,
        });
        // skip whitespace, then a comma or the closing bracket
        while i < val_end && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
            i += 1;
        }
        if i < val_end && b[i] == b',' {
            i += 1;
        } else {
            break;
        }
    }
}

/// Scans one JSON element (object/array/string/raw) from `i`, returning
/// the index just past it. Bracket/string tracking only — no parse.
// make scan_element_end reachable from the transform module's array walker
pub(crate) fn scan_element_end(b: &[u8], i: usize, limit: usize) -> usize {
    let mut j = i;
    match b[j] {
        b'"' => {
            j += 1;
            while j < limit {
                match b[j] {
                    b'\\' => j += 2,
                    b'"' => return j + 1,
                    _ => j += 1,
                }
            }
            j
        }
        b'{' | b'[' => {
            let mut stack: Vec<u8> = vec![b[j]];
            j += 1;
            while j < limit {
                let c = b[j];
                if *stack.last().expect("non-empty") == b'"' {
                    match c {
                        b'\\' => j += 2,
                        b'"' => {
                            stack.pop();
                            j += 1;
                        }
                        _ => j += 1,
                    }
                } else {
                    match c {
                        b'"' => {
                            stack.push(b'"');
                            // Advance past the opening quote: re-reading it
                            // with the string state on top would pop that
                            // state immediately, leaving the string's
                            // contents unprotected (a brace inside a payload
                            // string then corrupts the element span — the
                            // I2 fixture's grep payload found it).
                            j += 1;
                        }
                        b'{' | b'[' => {
                            stack.push(c);
                            j += 1;
                        }
                        b'}' | b']' => {
                            stack.pop();
                            j += 1;
                            if stack.is_empty() {
                                return j;
                            }
                        }
                        _ => j += 1,
                    }
                }
            }
            j
        }
        _ => {
            while j < limit && !matches!(b[j], b',' | b']' | b'}' | b' ' | b'\t' | b'\n' | b'\r') {
                j += 1;
            }
            j
        }
    }
}

/// GAP-Q14: attribute the **measured** `usage.input_total` over the blocks
/// proportionally by byte length (integer, remainder to the earliest
/// blocks). The figure is `inferred` by construction; hashes are the only
/// measured fidelity signal.
pub fn attribute_tokens(blocks: &mut [PrefixBlock], usage: &Usage) {
    if blocks.is_empty() || usage.input_total == 0 {
        return;
    }
    let total_bytes: u128 = blocks.iter().map(|b| b.byte_len as u128).sum();
    if total_bytes == 0 {
        return;
    }
    let n = blocks.len();
    let mut assigned: u64 = 0;
    for (i, blk) in blocks.iter_mut().enumerate() {
        let exact = usage.input_total as u128 * blk.byte_len as u128;
        let floor = (exact / total_bytes) as u64;
        // Floor for all but the last block; the last takes the remainder so
        // the attribution sums to exactly input_total.
        blk.tokens = if i + 1 == n {
            usage.input_total - assigned
        } else {
            assigned += floor;
            floor
        };
    }
}

/// Longest common block-prefix ratio (spec §6 `prefix_continuity`):
/// compares hashes from block 0. `None` when there is no previous request
/// in the session — an absent measurement is absent, not 1.0 and not 0.0.
///
/// A derived diagnostic ratio (spec §6 `prefix_continuity`), not a money path;
/// ADR-006's integer-only rule governs accounting, not this diagnostic.
#[allow(clippy::float_arithmetic)]
pub fn prefix_continuity(prev: &[PrefixBlock], cur: &[PrefixBlock]) -> Option<f64> {
    if prev.is_empty() || cur.is_empty() {
        return None;
    }
    let common = prev
        .iter()
        .zip(cur.iter())
        .take_while(|(a, b)| a.hash == b.hash)
        .count();
    // Spec §6: the ratio is *relative to the previous request* — the
    // denominator is its block count. A growing conversation (the
    // stateless-client shape: turn N's blocks are turn N-1's plus new
    // ones, per the captured traffic) therefore preserves 1.0, which is
    // exactly the cache-fidelity claim CONF-15 measures.
    let denom = prev.len();
    Some(common as f64 / denom as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha16_shape() {
        let h = body_sha16(b"abc");
        assert_eq!(h.len(), 16);
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
        // Deterministic (pure function of bytes).
        assert_eq!(h, body_sha16(b"abc"));
        assert_ne!(h, body_sha16(b"abd"));
    }

    #[test]
    fn extracts_in_template_order() {
        let body = RawBody::new(
            b"{\"system\":\"be brief\",\"model\":\"m\",\"messages\":[{\"role\":\"user\",\"content\":\"hi\"},{\"role\":\"assistant\",\"content\":\"yo\"}],\"tools\":[{\"name\":\"t1\"}],\"temperature\":0.7}"
                .to_vec(),
        );
        let blocks = extract_prefix_blocks(&body).unwrap();
        let kinds: Vec<&str> = blocks.iter().map(|b| b.kind.as_str()).collect();
        // Template order (CONF-31): system first, then tools, then the
        // conversation — regardless of body byte order (tools was
        // serialized after messages here).
        assert_eq!(kinds, vec!["system", "tool", "message", "message"]);
        assert!(blocks.iter().all(|b| b.hash.len() == 16));
    }

    /// CONF-31's core shape: the measured codex body serializes `input`
    /// before `tools`; turn 2 appends items at the `input` tail. In the
    /// provider's template order that is a pure tail append, so continuity
    /// must be exactly 1.0 — the regression this order change prevents
    /// (byte-order enumeration reported 0.250 on the real pair while the
    /// upstream verified hit rate was 0.991).
    #[test]
    fn codex_tail_append_is_full_continuity() {
        fn body(input: &str) -> RawBody {
            RawBody::new(
                format!(
                    "{{\"model\":\"m\",\"input\":[{input}],\"tools\":[{{\"name\":\"t1\"}},{{\"name\":\"t2\"}}],\"prompt_cache_key\":\"s\"}}"
                )
                .into_bytes(),
            )
        }
        let turn1 = body(r#"{"type":"message","role":"user","content":"a"}"#);
        let turn2 = body(concat!(
            r#"{"type":"message","role":"user","content":"a"},"#,
            r#"{"type":"message","role":"assistant","content":"b"},"#,
            r#"{"type":"message","role":"user","content":"c"}"#
        ));
        let b1 = extract_prefix_blocks(&turn1).unwrap();
        let b2 = extract_prefix_blocks(&turn2).unwrap();
        // Enumerated in template order: tools first, then input items.
        let kinds: Vec<&str> = b1.iter().map(|b| b.kind.as_str()).collect();
        assert_eq!(kinds, vec!["tool", "tool", "input_item"]);
        // The input tail append is a true tail append in this order.
        assert_eq!(prefix_continuity(&b1, &b2), Some(1.0));
    }

    /// The metric is bidirectionally movable, not a constant (CONF-31's
    /// companion): mutating turn 1's *first input item* — the front of the
    /// conversation — drops the ratio below 1.0 even though the tail and
    /// the tools are unchanged.
    #[test]
    fn mutated_first_input_item_drops_continuity() {
        fn body(first: &str, tail: &str) -> RawBody {
            RawBody::new(
                format!(
                    "{{\"model\":\"m\",\"input\":[{first},{tail}],\"tools\":[{{\"name\":\"t1\"}}]}}"
                )
                .into_bytes(),
            )
        }
        let a = extract_prefix_blocks(&body(
            r#"{"type":"message","role":"user","content":"original"}"#,
            r#"{"type":"message","role":"user","content":"tail"}"#,
        ))
        .unwrap();
        let b = extract_prefix_blocks(&body(
            r#"{"type":"message","role":"user","content":"REWRITTEN"}"#,
            r#"{"type":"message","role":"user","content":"tail"}"#,
        ))
        .unwrap();
        let c = prefix_continuity(&a, &b).expect("both turns have blocks");
        assert!(
            c < 1.0,
            "a first-item mutation must drop the ratio (got {c}); a constant 1.0 would be a broken metric"
        );
    }

    #[test]
    fn input_domain_uses_input_items() {
        let body = RawBody::new(
            b"{\"input\":[{\"role\":\"user\",\"content\":\"a\"},{\"type\":\"function_call\"}]}"
                .to_vec(),
        );
        let blocks = extract_prefix_blocks(&body).unwrap();
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].kind, BlockKind::InputItem);
        assert_ne!(blocks[0].hash, blocks[1].hash);
    }

    /// CONF-10's claim at block level: deleting vadis-owned fields cannot
    /// change any block hash (the domain excludes them).
    #[test]
    fn vadis_field_removal_does_not_change_block_hashes() {
        use crate::body::VADIS_OWNED_TOP_LEVEL_KEYS;
        let with_meta = RawBody::new(
            b"{\"vadis_meta\":{\"echo\":true},\"messages\":[{\"role\":\"user\",\"content\":\"hi\"}],\"tools\":[{\"name\":\"t\"}]}"
                .to_vec(),
        );
        let cleaned = with_meta
            .remove_top_level_keys(VADIS_OWNED_TOP_LEVEL_KEYS)
            .unwrap();
        let a = extract_prefix_blocks(&with_meta).unwrap();
        let b = extract_prefix_blocks(&cleaned).unwrap();
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.hash, y.hash, "block hash must not move");
        }
    }

    #[test]
    fn token_attribution_sums_to_input_total() {
        let mut blocks = extract_prefix_blocks(&RawBody::new(
            b"{\"messages\":[{\"a\":1},{\"b\":22},{\"c\":333}]}".to_vec(),
        ))
        .unwrap();
        attribute_tokens(
            &mut blocks,
            &Usage {
                input_total: 1000,
                input_cached: 0,
                cache_write: 0,
                output: 0,
                reasoning: 0,
            },
        );
        let sum: u64 = blocks.iter().map(|b| b.tokens).sum();
        assert_eq!(sum, 1000, "attribution must sum to input_total");
        // Proportional by byte length: the largest block takes the most.
        assert!(blocks[2].tokens > blocks[0].tokens);
    }

    #[test]
    fn continuity_common_prefix() {
        let a = extract_prefix_blocks(&RawBody::new(
            b"{\"messages\":[{\"a\":1},{\"b\":2},{\"c\":3}]}".to_vec(),
        ))
        .unwrap();
        let b = extract_prefix_blocks(&RawBody::new(
            b"{\"messages\":[{\"a\":1},{\"b\":2},{\"d\":4}]}".to_vec(),
        ))
        .unwrap();
        assert_eq!(prefix_continuity(&a, &b), Some(2.0 / 3.0));
        assert_eq!(prefix_continuity(&a, &a), Some(1.0));
        assert_eq!(prefix_continuity(&[], &b), None);
        // The stateless-client shape (CONF-15's object): turn N's blocks
        // are turn N-1's plus new ones — the ratio stays 1.0 because the
        // denominator is the *previous* request's block count (spec §6).
        let mut grown = a.clone();
        grown.push(PrefixBlock {
            index: 3,
            kind: BlockKind::Message,
            hash: "dddddddddddddddd".into(),
            byte_len: 9,
            tokens: 0,
        });
        assert_eq!(prefix_continuity(&a, &grown), Some(1.0));
        assert_eq!(prefix_continuity(&grown, &a), Some(3.0 / 4.0));
    }
}
