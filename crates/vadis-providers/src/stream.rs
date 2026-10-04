//! The streaming upstream attempt: one `open` call yields the
//! response head first, then `read_chunk` yields the upstream's body bytes
//! as they arrive. This layer still **makes no decisions** — it surfaces
//! the head-vs-bytes boundary the proxy's R6 semantics turn on.
//!
//! Byte rules (DESIGN §12.10.3): no `accept-encoding` is offered and no
//! transparent decompression happens (§12.10.3 R9); redirects are not
//! followed (§12.10.3 R10); the body bytes are the upstream's own, never
//! re-framed (§12.10.3 R1).

use std::time::Duration;

use bytes::Bytes;

use crate::UpstreamPlan;

/// A streaming attempt's head, before any body byte is read.
pub struct StreamHead {
    pub status: u16,
    pub retry_after: Option<String>,
    /// The response's content-type, relayed to the client verbatim on the
    /// native path.
    pub content_type: Option<String>,
    /// The upstream response with the body still unread.
    pub response: reqwest::Response,
}

impl StreamHead {
    /// The buffered-path view of the head (status + selected headers +
    /// the answer's own body bytes) — what error classification
    /// consumes on a failure head (DESIGN §12.10.3 R12). The body is
    /// read to the end, one `read_chunk` at a time, under §12.10.3
    /// R4's idle bound — no byte cap, matching the buffered path's
    /// own read; a read that fails or trips the bound simply yields
    /// the bytes that arrived. Consumes the head: a failure head has
    /// no second reader, and the success path relays the body instead
    /// of reading it.
    pub async fn into_upstream_response(
        mut self,
        idle_timeout: Duration,
    ) -> crate::UpstreamResponse {
        let status = self.status;
        let retry_after = self.retry_after.clone();
        let content_type = self.content_type.clone();
        let mut body = Vec::new();
        // Both `Failed` shapes (a mid-body read error and the idle
        // bound) end the loop like `Ended`: a read that ends short is
        // fewer evidence bytes, never an outcome of its own (the
        // classification's input, R12).
        while let StreamRead::Chunk(c) = read_chunk(&mut self, idle_timeout).await {
            body.extend_from_slice(&c);
        }
        crate::UpstreamResponse {
            status,
            retry_after,
            content_type,
            body: Bytes::from(body),
        }
    }
}

/// One read from the upstream body stream.
#[derive(Debug)]
pub enum StreamRead {
    /// Upstream bytes, in arrival order (chunk boundaries are the
    /// transport's, not a vadis framing).
    Chunk(Bytes),
    /// The upstream closed the body: the stream ended on its own terms.
    Ended,
    /// The body failed mid-stream (read error or the §12.10.3 R4 idle bound).
    /// `timed_out` distinguishes the §12.10.3 R4 idle case for the trace.
    Failed { message: String, timed_out: bool },
}

/// What `open` reports: the head arrived, or the attempt died before any
/// head (classify + fail over per ADR-011 — nothing has been relayed).
pub enum StreamOpen {
    Head(StreamHead),
    /// No request bytes went out. The transport kind is the
    /// classification evidence (the buffered and streaming
    /// paths feed the classifier the same inputs, so a connect failure is
    /// `connect_failure` on both).
    NotSent(crate::TransportKind, vadis_core::error::ErrorCode, String),
}

/// The streaming transport (DESIGN §12.10.1's seam, stream side). One
/// instance per provider, built at startup like the buffered client.
pub struct ReqwestStreamClient {
    client: reqwest::Client,
    /// §12.10.3 R4's bound: no upstream bytes for this long (after the head, or
    /// before it) is a failed relay. `server.upstream_attempt_timeout`.
    pub idle_timeout: Duration,
}

impl ReqwestStreamClient {
    pub fn new(idle_timeout: Duration) -> Result<Self, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(idle_timeout)
            .redirect(reqwest::redirect::Policy::none())
            // R9: no accept-encoding, no transparent decompression — a
            // compressed body travels as opaque bytes with its header.
            .no_brotli()
            .no_deflate()
            .no_gzip()
            .no_zstd()
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            client,
            idle_timeout,
        })
    }

    /// Sends the request and resolves with the head, bounded by
    /// `idle_timeout`. The outbound request is built by the buffered
    /// path's `build_request` (URL assembly + auth + the byte-faithful
    /// body), so the two paths can never disagree about its shape.
    pub async fn open(&self, plan: &UpstreamPlan<'_>, body: &[u8]) -> Result<StreamOpen, String> {
        let req = crate::build_request(plan, body)?;
        let (parts, body) = req.into_parts();
        let url = parts.uri.to_string();
        let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())
            .unwrap_or(reqwest::Method::POST);
        let mut out = self.client.request(method, &url);
        for (name, value) in parts.headers.iter() {
            out = out.header(name.as_str(), value.as_bytes());
        }
        let fut = out.body(body).timeout(self.idle_timeout).send();
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
                Ok(StreamOpen::Head(StreamHead {
                    status,
                    retry_after,
                    content_type,
                    response: resp,
                }))
            }
            Err(e) => {
                let wrote_full_request = !e.is_connect() && !e.is_request();
                if wrote_full_request {
                    // Full write, no head: the caller must treat this as
                    // `unknown_outcome` (ADR-011 item 6 row 3) — no retry,
                    // no failover. Surfaced as an Err so the proxy cannot
                    // mistake it for a classifiable pre-head failure.
                    Err(format!(
                        "request fully written but no response head (unknown_outcome): {e}"
                    ))
                } else {
                    let kind = if e.is_timeout() {
                        crate::TransportKind::Timeout
                    } else if e.is_connect() {
                        crate::TransportKind::Connect
                    } else {
                        crate::TransportKind::Other
                    };
                    Ok(StreamOpen::NotSent(
                        kind,
                        vadis_core::error::ErrorCode::UpstreamError,
                        e.to_string(),
                    ))
                }
            }
        }
    }
}

/// Reads the next upstream body chunk under §12.10.3 R4's idle bound. The bound
/// maps to `StreamRead::Failed { timed_out: true }` — the relay ends, it
/// never hangs.
pub async fn read_chunk(head: &mut StreamHead, idle_timeout: Duration) -> StreamRead {
    let mut body = std::pin::pin!(head.response.chunk());
    match tokio::time::timeout(idle_timeout, &mut body).await {
        // Layers: timeout(Elapsed) → the chunk result (reqwest::Error) →
        // the option (None = upstream closed the body).
        Ok(Ok(Some(chunk))) => StreamRead::Chunk(chunk),
        Ok(Ok(None)) => StreamRead::Ended,
        Ok(Err(e)) => StreamRead::Failed {
            message: e.to_string(),
            timed_out: false,
        },
        Err(_) => StreamRead::Failed {
            message: "idle bound exceeded: no upstream bytes within upstream_attempt_timeout"
                .into(),
            timed_out: true,
        },
    }
}
