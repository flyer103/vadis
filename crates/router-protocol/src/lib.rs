//! 3-protocol codec + translation matrix + `Usage` normalization + `raw_json`
//! (span-faithful editing). DESIGN §7 / §12.3.1.
//!
//! `Usage` normalization for all three wire shapes → the internal `Usage`,
//! on both the buffered path and the streaming one (`sse::SseUsageExtractor`
//! applies the protocols' own carrier rules). Translation between wire
//! shapes is not implemented in v0.1; only native routes are served.
//! `raw_json` span-faithful editing lives in `router-core::body`
//! (ADR-007's single scanner).

#![forbid(unsafe_code)]

pub mod sse;

use router_core::Usage;

/// Normalizes a chat-completions usage object (spec §2 usage row:
/// `usage.prompt/completion_tokens`, OpenAI-style
/// `prompt_tokens_details.cached_tokens`).
pub fn usage_from_chat(v: &serde_json::Value) -> Option<Usage> {
    let u = v.get("usage")?;
    let prompt = u.get("prompt_tokens")?.as_u64()?;
    let completion = u.get("completion_tokens")?.as_u64()?;
    let cached = u
        .get("prompt_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0);
    let reasoning = u
        .get("completion_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    Some(Usage {
        input_total: prompt,
        input_cached: cached.min(prompt),
        cache_write: 0,
        output: completion,
        reasoning,
    })
}

/// Normalizes a responses-api usage object
/// (`usage.input_tokens_details.cached_tokens` etc.).
pub fn usage_from_responses(v: &serde_json::Value) -> Option<Usage> {
    let u = v.get("usage")?;
    let input = u.get("input_tokens")?.as_u64()?;
    let output = u.get("output_tokens")?.as_u64()?;
    let cached = u
        .get("input_tokens_details")
        .and_then(|d| d.get("cached_tokens"))
        .and_then(|c| c.as_u64())
        .unwrap_or(0);
    let reasoning = u
        .get("output_tokens_details")
        .and_then(|d| d.get("reasoning_tokens"))
        .and_then(|r| r.as_u64())
        .unwrap_or(0);
    Some(Usage {
        input_total: input,
        input_cached: cached.min(input),
        cache_write: 0,
        output,
        reasoning,
    })
}

/// Normalizes an anthropic-messages usage object
/// (`usage.input_tokens/cache_read_input_tokens/cache_creation_input_tokens`).
pub fn usage_from_anthropic(v: &serde_json::Value) -> Option<Usage> {
    let u = v.get("usage")?;
    let input = u.get("input_tokens")?.as_u64()?;
    let output = u.get("output_tokens")?.as_u64()?;
    let cached = u
        .get("cache_read_input_tokens")
        .and_then(|c| c.as_u64())
        .unwrap_or(0);
    let cache_write = u
        .get("cache_creation_input_tokens")
        .and_then(|c| c.as_u64())
        .unwrap_or(0);
    Some(Usage {
        input_total: input,
        // Anthropic reports the cache-read tokens inside input_tokens; the
        // normalized shape counts them as the cached share of the input.
        input_cached: cached.min(input),
        cache_write,
        output,
        reasoning: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn chat_normalization() {
        let v = json!({
            "usage": {
                "prompt_tokens": 14409, "completion_tokens": 111,
                "prompt_tokens_details": {"cached_tokens": 14400},
                "completion_tokens_details": {"reasoning_tokens": 7}
            }
        });
        let u = usage_from_chat(&v).unwrap();
        assert_eq!(u.input_total, 14409);
        assert_eq!(u.input_cached, 14400);
        assert_eq!(u.output, 111);
        assert_eq!(u.reasoning, 7);
        assert_eq!(u.cache_write, 0);
    }

    #[test]
    fn responses_normalization() {
        let v = json!({
            "usage": {
                "input_tokens": 1000, "output_tokens": 50,
                "input_tokens_details": {"cached_tokens": 900},
                "output_tokens_details": {"reasoning_tokens": 10}
            }
        });
        let u = usage_from_responses(&v).unwrap();
        assert_eq!(u.input_total, 1000);
        assert_eq!(u.input_cached, 900);
        assert_eq!(u.output, 50);
        assert_eq!(u.reasoning, 10);
    }

    #[test]
    fn anthropic_normalization() {
        let v = json!({
            "usage": {
                "input_tokens": 2000, "output_tokens": 300,
                "cache_read_input_tokens": 1800, "cache_creation_input_tokens": 100
            }
        });
        let u = usage_from_anthropic(&v).unwrap();
        assert_eq!(u.input_total, 2000);
        assert_eq!(u.input_cached, 1800);
        assert_eq!(u.cache_write, 100);
        assert_eq!(u.output, 300);
    }

    /// Spec §6/§8 honesty rule: no usage object ⇒ `None` ⇒ the caller
    /// records `usage_missing`, charges nothing, invents no cost.
    #[test]
    fn missing_usage_is_none() {
        assert!(usage_from_chat(&json!({"choices": []})).is_none());
        assert!(usage_from_responses(&json!({})).is_none());
        assert!(usage_from_anthropic(&json!({"content": []})).is_none());
    }

    #[test]
    fn cached_never_exceeds_input() {
        let v = json!({"usage": {"input_tokens": 10, "output_tokens": 1,
            "cache_read_input_tokens": 99999}});
        assert_eq!(usage_from_anthropic(&v).unwrap().input_cached, 10);
    }
}
