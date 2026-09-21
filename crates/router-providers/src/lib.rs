//! `ProviderClient` (DESIGN §12.10.1, ADR-011): wire capabilities, auth,
//! one attempt per call. This layer **makes no decisions** — it surfaces
//! the raw material of a classification and hands it up.
//!
//! Both attempt shapes live here: `send` buffers the whole response body
//! before returning, `stream::open` + `read_chunk` yield the response head
//! first and then the upstream's body bytes as they arrive. A `stream: true`
//! inbound request is served by the SSE relay; the buffered engine answers it
//! `501`, never half-relayed.

#![forbid(unsafe_code)]

pub mod stream;

use std::time::Duration;

use bytes::Bytes;
use router_core::config::WireApi;

/// The outbound attempt's parameters (DESIGN §12.10.1 `UpstreamPlan`,
/// adapted: the body bytes are passed to `build` directly so the caller
/// owns the encoder's byte-final output).
pub struct UpstreamPlan<'a> {
    /// `provider/model` (the selected route).
    pub provider: &'a str,
    pub model: &'a str,
    /// The provider's `wire_api` — for a native route it equals the inbound
    /// protocol by construction.
    pub protocol_out: WireApi,
    /// The complete URL for this attempt's wire, resolved from the provider
    /// entry's `urls` map (spec §4.9, ADR-020). It is used verbatim: this
    /// layer composes no path, trims no slash and normalizes nothing.
    pub url: &'a str,
    /// The bearer/x-api-key value, from `api_key_env` at startup.
    pub api_key: &'a str,
    /// 0-based attempt index within one inbound request.
    pub attempt: u32,
}

/// A transport-level failure (no HTTP status). `wrote_full_request` is
/// ADR-011 item 6's flag: after a full write the upstream may already have
/// billed, so the attempt is `unknown_outcome` — not retried, not failed
/// over, quota not re-charged.
#[derive(Debug)]
pub struct TransportError {
    pub kind: TransportKind,
    pub message: String,
    pub wrote_full_request: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    /// Connect/TLS failure before any request byte went out.
    Connect,
    /// Timeout on the attempt (connect or read).
    Timeout,
    /// Anything else (body decode, protocol).
    Other,
}

/// The buffered upstream answer.
pub struct UpstreamResponse {
    pub status: u16,
    /// Selected headers the pipeline needs (`Retry-After`, content type).
    pub retry_after: Option<String>,
    /// The response's content-type, forwarded to the client verbatim on
    /// the native path.
    pub content_type: Option<String>,
    pub body: Bytes,
}

/// One attempt's outcome (DESIGN §12.10.1).
pub enum AttemptOutcome {
    /// An answer (any status) arrived.
    Responded(UpstreamResponse),
    /// No request bytes went out — nothing billed.
    NotSent(TransportError),
    /// Full write, no response: ADR-010 item 4's crash window.
    WrittenNoResponse(TransportError),
}

impl AttemptOutcome {
    /// `true` when the request bytes were fully written before the failure
    /// (ADR-011 item 6's retry rule turns on exactly this).
    pub fn wrote_full_request(&self) -> bool {
        matches!(self, Self::Responded(_) | Self::WrittenNoResponse(_))
    }
}

/// Builds the outbound request: the configured URL verbatim, auth headers,
/// the byte-faithful body. Pure — no I/O. The URL is **not** assembled here
/// any more (ADR-020): the provider entry names each wire's endpoint in full,
/// and this layer sends that string as written.
///
/// The body is the encoder's output: for a native route, the client's own
/// bytes minus the router-owned top-level keys (`RawBody`'s single
/// permitted rewrite), everything else verbatim (AGENTS constraint 1).
pub fn build_request(plan: &UpstreamPlan<'_>, body: &[u8]) -> Result<http::Request<Bytes>, String> {
    // No assembly: the plan carries the complete URL the config declared for
    // this wire (spec §4.9, ADR-020), and it is sent byte-for-byte as written.
    // A trailing slash is left alone — the router does not silently repair
    // what the operator wrote.
    let mut builder = http::Request::post(plan.url).header("content-type", "application/json");
    // Auth: chat/responses send Authorization: Bearer; anthropic sends
    // x-api-key + anthropic-version. The value exists only in the outbound
    // request — never in a log line, trace field or event payload.
    builder = match plan.protocol_out {
        WireApi::Anthropic => builder
            .header("x-api-key", plan.api_key)
            .header("anthropic-version", "2023-06-01"),
        WireApi::Chat | WireApi::Responses => {
            builder.header("authorization", format!("Bearer {}", plan.api_key))
        }
    };
    builder
        .body(Bytes::copy_from_slice(body))
        .map_err(|e| e.to_string())
}

/// The reqwest-backed client: **one instance per provider**, built at
/// startup so the connection pool is reused (DESIGN §12.10.1).
pub struct ReqwestProviderClient {
    client: reqwest::Client,
    attempt_timeout: Duration,
}

impl ReqwestProviderClient {
    /// `attempt_timeout` is `server.upstream_attempt_timeout`; the inbound
    /// `request_timeout` bounds the whole request and is enforced by the
    /// caller, not by the client.
    pub fn new(attempt_timeout: Duration) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(attempt_timeout)
            .redirect(reqwest::redirect::Policy::none())
            // No accept-encoding, no transparent decompression (R9): a
            // compressed body travels as opaque bytes with its header.
            .no_brotli()
            .no_deflate()
            .no_gzip()
            .no_zstd()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            attempt_timeout,
        })
    }

    /// Sends one attempt. Never retries, never fails over (ADR-011: retry
    /// is a *decision*, taken above this layer). The timeout covers the
    /// whole attempt.
    pub async fn send(&self, req: http::Request<Bytes>) -> AttemptOutcome {
        let (parts, body) = req.into_parts();
        let url = parts.uri.to_string();
        let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::POST);
        let mut out = self.client.request(method, &url);
        for (name, value) in parts.headers.iter() {
            out = out.header(name.as_str(), value.as_bytes());
        }
        let fut = out.body(body).timeout(self.attempt_timeout).send();
        match fut.await {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let retry_after = resp
                    .headers()
                    .get(reqwest::header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                let content_type = resp
                    .headers()
                    .get(reqwest::header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string());
                match resp.bytes().await {
                    Ok(body) => AttemptOutcome::Responded(UpstreamResponse {
                        status,
                        retry_after,
                        content_type,
                        body,
                    }),
                    Err(e) => {
                        // The response body failed mid-read after the head:
                        // treat as written-no-response (the conservative
                        // ADR-011 item 6 arm).
                        AttemptOutcome::WrittenNoResponse(TransportError {
                            kind: TransportKind::Other,
                            message: format!("response body read failed: {e}"),
                            wrote_full_request: true,
                        })
                    }
                }
            }
            Err(e) => {
                let wrote_full_request = !e.is_connect() && !e.is_request();
                let kind = if e.is_timeout() {
                    TransportKind::Timeout
                } else if e.is_connect() {
                    TransportKind::Connect
                } else {
                    TransportKind::Other
                };
                if wrote_full_request {
                    AttemptOutcome::WrittenNoResponse(TransportError {
                        kind,
                        message: e.to_string(),
                        wrote_full_request: true,
                    })
                } else {
                    AttemptOutcome::NotSent(TransportError {
                        kind,
                        message: e.to_string(),
                        wrote_full_request: false,
                    })
                }
            }
        }
    }
}

/// The test double (DESIGN §12.10.1): returns a canned `AttemptOutcome`
/// and records the bytes it was handed. Every case that must not touch the
/// network uses it.
#[derive(Debug, Clone)]
pub struct FakeProvider {
    /// Queue of outcomes; the last entry repeats when the queue empties.
    pub outcomes: Vec<AttemptOutcomeDouble>,
    /// Every request this double was handed, in order.
    pub seen: std::sync::Arc<std::sync::Mutex<Vec<RecordedRequest>>>,
}

#[derive(Debug, Clone)]
pub enum AttemptOutcomeDouble {
    Responded { status: u16, body: Vec<u8> },
    NotSent,
    WrittenNoResponse,
}

#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeProvider {
    pub fn new() -> Self {
        Self {
            outcomes: Vec::new(),
            seen: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    pub fn respond(status: u16, body: &[u8]) -> Self {
        let mut f = Self::new();
        f.outcomes.push(AttemptOutcomeDouble::Responded {
            status,
            body: body.to_vec(),
        });
        f
    }

    pub async fn send(&self, req: http::Request<Bytes>) -> AttemptOutcome {
        let (parts, body) = req.into_parts();
        self.seen.lock().unwrap().push(RecordedRequest {
            url: parts.uri.to_string(),
            headers: parts
                .headers
                .iter()
                .map(|(k, v)| {
                    (
                        k.as_str().to_string(),
                        String::from_utf8_lossy(v.as_bytes()).into_owned(),
                    )
                })
                .collect(),
            body: body.to_vec(),
        });
        let next = self
            .outcomes
            .first()
            .cloned()
            .unwrap_or(AttemptOutcomeDouble::NotSent);
        match next {
            AttemptOutcomeDouble::Responded { status, body } => {
                AttemptOutcome::Responded(UpstreamResponse {
                    status,
                    retry_after: None,
                    content_type: Some("application/json".into()),
                    body: Bytes::from(body),
                })
            }
            AttemptOutcomeDouble::NotSent => AttemptOutcome::NotSent(TransportError {
                kind: TransportKind::Connect,
                message: "fake: connect refused".into(),
                wrote_full_request: false,
            }),
            AttemptOutcomeDouble::WrittenNoResponse => {
                AttemptOutcome::WrittenNoResponse(TransportError {
                    kind: TransportKind::Timeout,
                    message: "fake: written, no response".into(),
                    wrote_full_request: true,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan<'a>() -> UpstreamPlan<'a> {
        UpstreamPlan {
            provider: "p",
            model: "m",
            protocol_out: WireApi::Chat,
            url: "https://p.example/v1/chat/completions",
            api_key: "sk-test",
            attempt: 0,
        }
    }

    #[test]
    fn url_is_the_configured_one_and_auth_chat() {
        let req = build_request(&plan(), b"{}").unwrap();
        assert_eq!(
            req.uri().to_string(),
            "https://p.example/v1/chat/completions"
        );
        assert_eq!(
            req.headers().get("authorization").unwrap(),
            "Bearer sk-test"
        );
        assert_eq!(
            req.headers().get("content-type").unwrap(),
            "application/json"
        );
    }

    #[test]
    fn url_is_the_configured_one_and_auth_anthropic() {
        let mut p = plan();
        p.protocol_out = WireApi::Anthropic;
        // The vendor serves the Anthropic form at a base of its own, so the
        // entry states the whole thing and nothing is appended to it.
        p.url = "https://p.example/anthropic/v1/messages";
        let req = build_request(&p, b"{}").unwrap();
        assert_eq!(
            req.uri().to_string(),
            "https://p.example/anthropic/v1/messages"
        );
        assert_eq!(req.headers().get("x-api-key").unwrap(), "sk-test");
        assert_eq!(
            req.headers().get("anthropic-version").unwrap(),
            "2023-06-01"
        );
        assert!(req.headers().get("authorization").is_none());
    }

    #[test]
    fn a_configured_url_is_sent_verbatim_even_with_a_trailing_slash() {
        // ADR-020 item 1: no composition and no normalization. A slash the
        // operator wrote is a slash the upstream sees — which is also why the
        // URL is validated as absolute at load time instead of being repaired
        // here.
        let mut p = plan();
        p.url = "https://p.example/v1/chat/completions/";
        let req = build_request(&p, b"{}").unwrap();
        assert_eq!(
            req.uri().to_string(),
            "https://p.example/v1/chat/completions/"
        );
    }

    #[tokio::test]
    async fn fake_records_bytes_and_canned_response() {
        let fake = FakeProvider::respond(200, br#"{"ok":true}"#);
        let req = build_request(&plan(), b"{\"model\":\"m\"}").unwrap();
        match fake.send(req).await {
            AttemptOutcome::Responded(r) => {
                assert_eq!(r.status, 200);
                assert_eq!(&r.body[..], b"{\"ok\":true}");
            }
            _ => panic!("expected Responded"),
        }
        let seen = fake.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].body, b"{\"model\":\"m\"}");
        assert!(seen[0].url.ends_with("/chat/completions"));
    }
}
