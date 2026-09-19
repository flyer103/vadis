//! SSE-carried usage extraction (DESIGN §12.10.3 R8): the accounting tap
//! that reads a **copy** of the relayed bytes and reports the usage the
//! protocol's own carrier delivered — never a guess.
//!
//! This module never touches the relay path: it consumes the same bytes
//! through `feed` and answers `finish`. A parse failure on any event is
//! ignored (keep-alives, comments and non-JSON payloads are normal SSE
//! content); the tap degrades to `Missing`, it never panics and never
//! invents a number.
//!
//! Carrier rules (R8):
//! - `chat`: usage exists in the stream only when the client asked for it
//!   (`stream_options.include_usage`) — the extractor is told at
//!   construction; `[DONE]` is the terminal marker.
//! - `responses`: the terminal events (`response.completed`,
//!   `response.failed`, `response.incomplete`) carry `response.usage`.
//! - `anthropic`: `message_start` carries the input side of
//!   `message.usage`, `message_delta` the (cumulative) output side;
//!   `message_stop` is the terminal marker.

use super::{usage_from_anthropic, usage_from_chat, usage_from_responses};
use router_core::config::WireApi;
use router_core::Usage;

/// What `finish` reports: the protocol carried a usage, or it did not.
/// `Missing` is the honest `usage_missing: true` of spec §8 — zero usage,
/// nothing charged, nothing invented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SseUsageOutcome {
    Missing,
    Complete(Usage),
}

/// An incremental, allocation-light tap over the relayed byte sequence.
///
/// Feed it every chunk in arrival order (chunk boundaries are irrelevant:
/// events are reassembled from the byte stream); call `finish` once the
/// relay ends.
pub struct SseUsageExtractor {
    wire: WireApi,
    /// chat only: the client asked for `stream_options.include_usage`.
    chat_carrier_allowed: bool,
    /// Bytes of a not-yet-complete event (carried across chunks).
    pending: Vec<u8>,
    /// The input-side usage (anthropic `message_start`).
    input_side: Option<Usage>,
    /// The output-side usage (anthropic `message_delta`).
    output_side: Option<Usage>,
    /// A full usage carried by one event (chat / responses terminal).
    whole: Option<Usage>,
    terminal_seen: bool,
}

impl SseUsageExtractor {
    pub fn new(wire: WireApi, client_requested_usage: bool) -> Self {
        Self {
            wire,
            // Only chat gates the carrier on the client's request; the
            // other two protocols always carry usage in their terminals.
            chat_carrier_allowed: wire != WireApi::Chat || client_requested_usage,
            pending: Vec::new(),
            input_side: None,
            output_side: None,
            whole: None,
            terminal_seen: false,
        }
    }

    /// Feed one relayed chunk. Reassembles complete SSE events (blank-line
    /// terminated) and harvests each protocol's usage carrier.
    pub fn feed(&mut self, chunk: &[u8]) {
        self.pending.extend_from_slice(chunk);
        while let Some((end, term_len)) = next_event_span(&self.pending) {
            // `end` indexes the blank-line terminator: everything before it
            // is one complete event body.
            let mut event: Vec<u8> = self.pending.drain(..end + term_len).collect();
            event.truncate(end);
            self.observe_event(&event);
        }
    }

    /// The tap's verdict. Call once the relay ended.
    pub fn finish(&self) -> SseUsageOutcome {
        if !self.terminal_seen {
            // A stream that never reached its terminal event reports no
            // usage, even if a usage event happened to arrive earlier —
            // R11: an incomplete stream is marked, never half-accounted.
            return SseUsageOutcome::Missing;
        }
        let usage = match self.wire {
            WireApi::Chat | WireApi::Responses => self.whole,
            WireApi::Anthropic => self.merged_anthropic(),
        };
        // A protocol with no usage carrier this request reports Missing —
        // the caller records `usage_missing: true` (spec §8), never a 0.
        match usage {
            Some(u) if self.chat_carrier_allowed => SseUsageOutcome::Complete(u),
            _ => SseUsageOutcome::Missing,
        }
    }

    fn merged_anthropic(&self) -> Option<Usage> {
        // message_delta's usage is the later, cumulative one; where it
        // repeats an input field it wins over message_start.
        let base = self.input_side?;
        let out = self.output_side.unwrap_or(base);
        Some(Usage {
            input_total: if out.input_total > 0 {
                out.input_total
            } else {
                base.input_total
            },
            input_cached: out.input_cached.max(base.input_cached),
            cache_write: out.cache_write.max(base.cache_write),
            output: out.output,
            reasoning: out.reasoning,
        })
    }

    /// Anthropic's `message_delta` carries a *partial* usage (typically
    /// `output_tokens` only), which the buffered-path normalizer would
    /// reject (it requires the full shape). Parse leniently: absent fields
    /// are 0 and only fields actually present count.
    fn anthropic_partial_usage(v: &serde_json::Value) -> Option<Usage> {
        let u = v.get("usage")?;
        let field = |name: &str| u.get(name).and_then(|x| x.as_u64()).unwrap_or(0);
        let input = field("input_tokens");
        let output = field("output_tokens");
        if u.get("input_tokens").is_none() && u.get("output_tokens").is_none() {
            return None; // a usage object with neither field is no carrier
        }
        Some(Usage {
            input_total: input,
            input_cached: field("cache_read_input_tokens").min(input.max(1)),
            cache_write: field("cache_creation_input_tokens"),
            output,
            reasoning: 0,
        })
    }

    /// Harvests one complete SSE event body (without the blank-line
    /// terminator). `event:`/`data:` lines only; anything else is skipped.
    fn observe_event(&mut self, event_body: &[u8]) {
        let mut event_name: Option<String> = None;
        let mut data = Vec::new();
        for line in split_lines(event_body) {
            if let Some(v) = line.strip_prefix(b"event:") {
                event_name = Some(String::from_utf8_lossy(trim_field(v)).into_owned());
            } else if let Some(v) = line.strip_prefix(b"data:") {
                // SSE joins multiple data lines with '\n'; usage carriers
                // are single-line JSON, but the join is spec-correct.
                if !data.is_empty() {
                    data.push(b'\n');
                }
                data.extend_from_slice(trim_field(v));
            }
            // `id:` / `retry:` / comments: not usage carriers, skipped.
        }
        if data.is_empty() {
            return;
        }
        match self.wire {
            WireApi::Chat => self.observe_chat(&data),
            WireApi::Responses => self.observe_responses(&data),
            WireApi::Anthropic => self.observe_anthropic(event_name.as_deref(), &data),
        }
    }

    fn observe_chat(&mut self, data: &[u8]) {
        if data == b"[DONE]" {
            self.terminal_seen = true;
            return;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) else {
            return; // keep-alive or comment payload; not a carrier
        };
        if v.get("usage").is_some() {
            // The final chunk's usage object; reuse the buffered-path
            // normalizer so stream and buffered agree field for field.
            self.whole = usage_from_chat(&v);
        }
    }

    fn observe_responses(&mut self, data: &[u8]) {
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) else {
            return;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("response.completed") | Some("response.failed") | Some("response.incomplete") => {
                self.terminal_seen = true;
                if let Some(u) = v.get("response").and_then(|r| r.get("usage")) {
                    let wrapper = serde_json::json!({ "usage": u });
                    self.whole = usage_from_responses(&wrapper);
                }
            }
            _ => {}
        }
    }

    fn observe_anthropic(&mut self, event_name: Option<&str>, data: &[u8]) {
        let Some(name) = event_name else { return };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(data) else {
            return;
        };
        match name {
            "message_start" => {
                if let Some(u) = v.get("message").and_then(|m| m.get("usage")) {
                    let wrapper = serde_json::json!({ "usage": u });
                    self.input_side = usage_from_anthropic(&wrapper);
                }
            }
            "message_delta" => {
                if let Some(u) = Self::anthropic_partial_usage(&v) {
                    self.output_side = Some(u);
                }
            }
            "message_stop" => self.terminal_seen = true,
            _ => {}
        }
    }
}

/// Finds the next complete event in `buf`: returns `(data_start, term_len)`
/// — the event body starts after any leading newlines carried over from
/// the previous split, and `term_len` is the blank-line terminator's
/// length (2 for `\n\n`, 4 for `\r\n\r\n`). `None` when no complete event
/// is buffered yet.
fn next_event_span(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = find(buf, b"\n\n").map(|i| (i, 2));
    let crlf = find(buf, b"\r\n\r\n").map(|i| (i, 4));
    match (lf, crlf) {
        (Some((a, la)), Some((b, lb))) => Some(if a < b { (a, la) } else { (b, lb) }),
        (Some((a, la)), None) => Some((a, la)),
        (None, Some((b, lb))) => Some((b, lb)),
        (None, None) => None,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Splits into `\n`-terminated lines, stripping a trailing `\r` (CRLF).
fn split_lines(mut bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    std::iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }
        let (line, rest) = match find(bytes, b"\n") {
            Some(i) => (&bytes[..i], &bytes[i + 1..]),
            None => (bytes, &[][..]),
        };
        bytes = rest;
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        Some(line)
    })
}

/// SSE field-value trim: exactly one optional leading space (the spec's
/// rule — a data payload that *starts* with a space keeps the second one).
fn trim_field(v: &[u8]) -> &[u8] {
    v.strip_prefix(b" ").unwrap_or(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHAT_STREAM: &[&str] = &[
        "data: {\"choices\":[{\"delta\":{\"content\":\"he\"}}]}\n\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"llo\"}}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":100,\"completion_tokens\":5,\"total_tokens\":105,\"prompt_tokens_details\":{\"cached_tokens\":80}}}\n\n",
        "data: [DONE]\n\n",
    ];

    fn feed_all(ex: &mut SseUsageExtractor, chunks: &[&str]) {
        for c in chunks {
            ex.feed(c.as_bytes());
        }
    }

    #[test]
    fn chat_usage_arrives_and_completes() {
        let mut ex = SseUsageExtractor::new(WireApi::Chat, true);
        feed_all(&mut ex, CHAT_STREAM);
        match ex.finish() {
            SseUsageOutcome::Complete(u) => {
                assert_eq!(u.input_total, 100);
                assert_eq!(u.input_cached, 80);
                assert_eq!(u.output, 5);
            }
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn chat_without_include_usage_has_no_carrier() {
        let mut ex = SseUsageExtractor::new(WireApi::Chat, false);
        feed_all(&mut ex, CHAT_STREAM);
        assert_eq!(ex.finish(), SseUsageOutcome::Missing);
    }

    #[test]
    fn chat_usage_but_no_done_marker_is_missing() {
        // Truncated after the usage chunk, before [DONE]: R11 says mark,
        // not half-account.
        let mut ex = SseUsageExtractor::new(WireApi::Chat, true);
        feed_all(&mut ex, &CHAT_STREAM[..3]);
        assert_eq!(ex.finish(), SseUsageOutcome::Missing);
    }

    #[test]
    fn chunk_boundaries_never_split_an_event_away() {
        // One byte at a time: the reassembly must be boundary-independent.
        let all = CHAT_STREAM.concat();
        let mut ex = SseUsageExtractor::new(WireApi::Chat, true);
        for b in all.bytes() {
            ex.feed(&[b]);
        }
        assert!(matches!(ex.finish(), SseUsageOutcome::Complete(_)));
    }

    #[test]
    fn crlf_framing_is_accepted() {
        let mut ex = SseUsageExtractor::new(WireApi::Chat, true);
        ex.feed(b"data: {\"usage\":{\"prompt_tokens\":7,\"completion_tokens\":2}}\r\n\r\n");
        ex.feed(b"data: [DONE]\r\n\r\n");
        match ex.finish() {
            SseUsageOutcome::Complete(u) => {
                assert_eq!(u.input_total, 7);
                assert_eq!(u.output, 2);
            }
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn responses_terminal_carries_usage() {
        let mut ex = SseUsageExtractor::new(WireApi::Responses, false);
        ex.feed(b"event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"usage\":{\"input_tokens\":14409,\"output_tokens\":111,\"input_tokens_details\":{\"cached_tokens\":14400}}}}\n\n");
        match ex.finish() {
            SseUsageOutcome::Complete(u) => {
                assert_eq!(u.input_total, 14409);
                assert_eq!(u.input_cached, 14400);
                assert_eq!(u.output, 111);
            }
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn anthropic_start_delta_stop_merge() {
        let mut ex = SseUsageExtractor::new(WireApi::Anthropic, false);
        ex.feed(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":25,\"cache_read_input_tokens\":20,\"cache_creation_input_tokens\":5,\"output_tokens\":1}}}\n\n");
        ex.feed(b"event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"delta\":{\"type\":\"text_delta\",\"text\":\"hi\"}}\n\n");
        ex.feed(b"event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":17}}\n\n");
        ex.feed(b"event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n");
        match ex.finish() {
            SseUsageOutcome::Complete(u) => {
                assert_eq!(u.input_total, 25);
                assert_eq!(u.input_cached, 20);
                assert_eq!(u.cache_write, 5);
                assert_eq!(u.output, 17);
            }
            _ => panic!("expected Complete"),
        }
    }

    #[test]
    fn anthropic_truncated_stream_is_missing() {
        let mut ex = SseUsageExtractor::new(WireApi::Anthropic, false);
        ex.feed(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":25,\"output_tokens\":1}}}\n\n");
        // connection dropped before message_stop
        assert_eq!(ex.finish(), SseUsageOutcome::Missing);
    }

    #[test]
    fn non_json_data_lines_are_ignored() {
        let mut ex = SseUsageExtractor::new(WireApi::Chat, true);
        ex.feed(b": keep-alive\n\n");
        ex.feed(b"data: not json at all\n\n");
        ex.feed(b"data: [DONE]\n\n");
        // Terminal seen, but no usage carrier arrived: honest Missing.
        assert_eq!(ex.finish(), SseUsageOutcome::Missing);
    }
}
