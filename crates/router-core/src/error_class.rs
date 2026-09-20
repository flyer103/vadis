//! Upstream error classification (ADR-011): one classifier, in one place,
//! as a pure function. `router-providers` surfaces the raw material and
//! decides nothing; the proxy, the guard chain and the plugins never branch
//! on an upstream error body's text.
//!
//! This round implements the **minimal class set the buffered failover path
//! needs** (the card's scope). The full v0.1 slice of ADR-011 item 1 (14
//! classes) lands with the paths that exercise it; classes without a table
//! entry and a fixture do not exist (ADR-011 item 1).

use std::time::Duration;

/// The ADR-011 classes this round's failover path consumes. The string
/// forms are stable (event payloads, cooldown `reason`, trace `details`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorClass {
    /// 401/403 without an exhaustion keyword: the key may be wrong or lack
    /// a permission. Retry on another provider.
    Auth,
    /// 429 (or 403 + a rate-limit keyword): transient throttling. The
    /// provider's own clock (`Retry-After`) is honored as cooldown state.
    RateLimit,
    /// 403 + an account-exhaustion keyword: a fact about the **account**.
    /// Demotes the whole provider (ADR-011 item 4), never a single model.
    QuotaExhausted,
    /// 5xx + overload wording. Fail over.
    Overloaded,
    /// Any other 5xx. Fail over.
    ServerError,
    /// A no-status failure whose transport evidence says no connection
    /// was ever established (ADR-011 item 6 row 1: nothing was billed,
    /// so the chain is walked). Distinct from [`ErrorClass::Timeout`]:
    /// conflating them pollutes the "why did we switch" evidence and
    /// puts the recovery action on the wrong class.
    ConnectFailure,
    /// The attempt timed out. Deterministic classes may not take this
    /// label (see [`ErrorEvidence::wrote_full_request`]).
    Timeout,
    /// 404 on the model (or a `model_not_found` code): a roster defect.
    /// Fail over; the roster entry is surfaced.
    ModelNotFound,
    /// A request-shape rejection (400, or a 5xx carrying request-validation
    /// text — ADR-011 item 2's ordering lesson): deterministic, **not
    /// retried** on another provider.
    FormatError,
    /// A content-policy refusal: deterministic for the unchanged request.
    /// Returned to the client as-is; **never retried** (ADR-011 item 3).
    ContentPolicyBlocked,
}

impl ErrorClass {
    /// The stable wire form (event payloads, cooldown reason, trace details).
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::RateLimit => "rate_limit",
            Self::QuotaExhausted => "quota_exhausted",
            Self::Overloaded => "overloaded",
            Self::ServerError => "server_error",
            Self::ConnectFailure => "connect_failure",
            Self::Timeout => "timeout",
            Self::ModelNotFound => "model_not_found",
            Self::FormatError => "format_error",
            Self::ContentPolicyBlocked => "content_policy_blocked",
        }
    }

    /// May another provider be attempted for this failure (spec §4.2)?
    /// The deterministic classes may not: a re-probe reproduces the same
    /// refusal and burns a paid attempt to learn nothing (ADR-011 item 3).
    pub const fn fails_over(self) -> bool {
        !matches!(
            self,
            Self::FormatError | Self::ContentPolicyBlocked | Self::Timeout
        )
    }

    /// Does this class demote the **whole provider** (ADR-011 item 4)?
    /// Only quota/billing exhaustion is a fact about the account; every
    /// other failure is a fact about the attempt or the route.
    pub const fn demotes_provider(self) -> bool {
        matches!(self, Self::QuotaExhausted)
    }
}

/// The demotion a classification may order (ADR-011 items 4–5). The scope
/// is the provider for billing-class failures, nothing for the rest — this
/// round writes no route-scoped cooldowns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Demotion {
    /// `true` = provider-wide (all of its models unavailable until the TTL).
    pub provider_wide: bool,
    /// How long the demotion holds. From the provider's own clock when it
    /// sent one (`Retry-After`), else the declared default cooldown
    /// (ADR-011 item 4).
    pub ttl: Duration,
}

/// The discriminable cause of a no-status (transport) failure, as the
/// transport layer derived it from the underlying error's kind — the
/// evidence source for the `connect_failure` vs `timeout` split. The
/// proxy maps its transport kind onto this; the classifier itself
/// never inspects error text (ADR-011 item 1: no scattered matching).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportCause {
    /// No connection was ever established (connect/TLS failure before
    /// the request bytes went out; reqwest `is_connect()`).
    Connect,
    /// The attempt timed out (reqwest `is_timeout()`).
    Timeout,
    /// Anything else (body decode, protocol). Without a status and
    /// without a narrower cause, the honest label is `timeout` only
    /// by the existing convention; ADR-011 item 2 rule 6's `Unknown`
    /// lands with the paths that exercise it.
    Other,
}

/// The raw material of a classification (DESIGN §12.10.1). Surfaced by the
/// provider layer, consumed only here.
pub struct ErrorEvidence<'a> {
    /// The upstream status when an answer arrived; `None` for a transport
    /// failure.
    pub status: Option<u16>,
    /// The response headers (`Retry-After` is read here).
    pub retry_after: Option<&'a str>,
    /// The error body bytes. Parsing the body is **never** a precondition:
    /// a body that is not JSON, or not UTF-8, still classifies on status
    /// and headers alone (ADR-011 item 10).
    pub body: &'a [u8],
    /// ADR-011 item 6: `true` when the request bytes were fully written
    /// before the failure (the read-timeout case). Such an attempt may have
    /// been billed: it is **not retried and not failed over** — the correct
    /// shape is `unknown_outcome`, discovered by reconciliation.
    pub wrote_full_request: bool,
    /// The transport-layer cause of a **no-status** failure. Read only on
    /// the no-status arm, where it decides `connect_failure` vs `timeout`;
    /// ignored whenever an answer arrived.
    pub transport_cause: Option<TransportCause>,
}

/// One classification verdict.
pub struct Classification {
    pub class: ErrorClass,
    /// The table entry that decided it (reproducibility, ADR-011 item 1).
    pub matched: &'static str,
    /// The provider's own clock, when it sent one (state, ADR-011 item 5).
    pub retry_after: Option<Duration>,
    /// The provider demotion this classification orders, if any.
    pub demotion: Option<Demotion>,
}

/// The declared default cooldown when the provider sends no reset instant
/// (ADR-011 item 4: "a declared default cooldown" — an honest default, not
/// a guess dressed as a measurement).
pub const DEFAULT_COOLDOWN: Duration = Duration::from_secs(60);

// --- the pattern tables (ADR-011 item 11: code, narrow verbatim strings) ---

/// Account-exhaustion wording (ADR-011 item 4's trigger list). Distinct
/// from rate-limit wording on purpose: the two have opposite TTLs.
const QUOTA_EXHAUSTED_PATTERNS: &[&str] = &[
    "insufficient_quota",
    "quota exceeded",
    "usage limit",
    "access_terminated",
    "plan does not include",
    "credits exhausted",
    "model_not_supported_on_free_tier",
];

/// Rate-limit wording (checked only when the status says 429).
const RATE_LIMIT_PATTERNS: &[&str] = &[
    "rate limit",
    "too many requests",
    "throttled",
    "resource_exhausted",
];

/// Overload wording (checked on 5xx).
const OVERLOADED_PATTERNS: &[&str] = &["overloaded", "overloaded_error", "server is at capacity"];

/// Content-policy refusal wording (checked first, whatever the status —
/// ADR-011 item 2 rule 1: a status-less or oddly-statused refusal must not
/// fall into a retryable catch-all).
const CONTENT_POLICY_PATTERNS: &[&str] = &[
    "content_policy_violation",
    "content policy",
    "content_policy",
];

/// Request-validation wording: a 5xx carrying it is `format_error`, not a
/// retryable server error (ADR-011 item 2's ordering lesson).
const FORMAT_ERROR_PATTERNS: &[&str] = &[
    "unknown parameter",
    "unsupported parameter",
    "invalid_request_error",
    "invalid request",
];

/// Case-insensitive verbatim substring match over the lossily-decoded body
/// (a body that is not UTF-8 classifies on status alone; the patterns are
/// ASCII so the lossy decode cannot manufacture a match).
fn body_contains(body: &[u8], needle: &str) -> bool {
    let hay = String::from_utf8_lossy(body).to_ascii_lowercase();
    hay.contains(&needle.to_ascii_lowercase())
}

fn first_match(body: &[u8], table: &'static [&'static str]) -> Option<&'static str> {
    table.iter().copied().find(|p| body_contains(body, p))
}

/// Parses `Retry-After` (delta-seconds or an HTTP-date). The HTTP-date arm
/// needs a clock, which a pure function does not own — a date form is
/// therefore **ignored** here (returns `None`); the caller's own state
/// layer may honor it in a later round. Never an error.
pub fn parse_retry_after(v: &str) -> Option<Duration> {
    let t = v.trim();
    if let Ok(secs) = t.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    None
}

/// The classifier (ADR-011 items 2–5). Pure: a function of the evidence
/// and nothing else — no clock, no RNG, no turn counter (AGENTS constraint 2).
pub fn classify_upstream_error(ev: &ErrorEvidence<'_>) -> Classification {
    let retry_after = ev.retry_after.and_then(parse_retry_after);

    // Rule 1: provider-specific narrow patterns first — a deterministic
    // refusal must not be downgraded to a generic status class.
    if let Some(m) = first_match(ev.body, CONTENT_POLICY_PATTERNS) {
        return Classification {
            class: ErrorClass::ContentPolicyBlocked,
            matched: m,
            retry_after: None,
            demotion: None,
        };
    }

    // A transport failure (no status): the transport cause decides
    // `connect_failure` (nothing was billed — ADR-011 item 6 row 1 —
    // so the class fails over and never re-attempts the same provider
    // in this request) vs `timeout` (the existing semantics: a
    // deterministic-class treatment, and when `wrote_full_request`
    // is set the caller must treat the attempt as `unknown_outcome`,
    // which it detects from that same flag — the class stays
    // `timeout`). A `None` cause keeps the pre-split `timeout`
    // verdict: the caller had no transport evidence to give.
    let Some(status) = ev.status else {
        let class = match ev.transport_cause {
            Some(TransportCause::Connect) => ErrorClass::ConnectFailure,
            Some(TransportCause::Timeout) | Some(TransportCause::Other) | None => {
                ErrorClass::Timeout
            }
        };
        return Classification {
            class,
            matched: "no-status",
            retry_after: None,
            demotion: None,
        };
    };

    let mut cls = classify_status(status, ev);

    // Rule 3: the structured `code`/`type` inside the body can refine a
    // status-derived class when the body carries one (checked for 4xx/5xx).
    if let Some(refined) = refine_by_body(status, ev) {
        cls = refined;
    }
    cls.retry_after = retry_after;
    if let Some(ra) = retry_after {
        if cls.class.demotes_provider() {
            cls.demotion = Some(Demotion {
                provider_wide: true,
                ttl: ra,
            });
        }
    } else if cls.class.demotes_provider() {
        cls.demotion = Some(Demotion {
            provider_wide: true,
            ttl: DEFAULT_COOLDOWN,
        });
    }
    cls
}

fn classify_status(status: u16, ev: &ErrorEvidence<'_>) -> Classification {
    match status {
        400 => Classification {
            class: ErrorClass::FormatError,
            matched: "400",
            retry_after: None,
            demotion: None,
        },
        401 | 402 => Classification {
            class: ErrorClass::Auth,
            matched: "401/402",
            retry_after: None,
            demotion: None,
        },
        403 => {
            // ADR-011 item 4: 403 + account-exhaustion wording is a
            // provider-level fact; otherwise it is an auth-class failure.
            if let Some(m) = first_match(ev.body, QUOTA_EXHAUSTED_PATTERNS) {
                Classification {
                    class: ErrorClass::QuotaExhausted,
                    matched: m,
                    retry_after: None,
                    demotion: None,
                }
            } else {
                Classification {
                    class: ErrorClass::Auth,
                    matched: "403",
                    retry_after: None,
                    demotion: None,
                }
            }
        }
        404 => Classification {
            class: ErrorClass::ModelNotFound,
            matched: "404",
            retry_after: None,
            demotion: None,
        },
        413 => Classification {
            class: ErrorClass::FormatError,
            matched: "413",
            retry_after: None,
            demotion: None,
        },
        429 => Classification {
            class: ErrorClass::RateLimit,
            matched: "429",
            retry_after: None,
            demotion: None,
        },
        s if (500..600).contains(&s) => Classification {
            class: ErrorClass::ServerError,
            matched: "5xx",
            retry_after: None,
            demotion: None,
        },
        // Any other answered status is treated as a request-shape problem:
        // deterministic, surfaced to the client.
        _ => Classification {
            class: ErrorClass::FormatError,
            matched: "other-status",
            retry_after: None,
            demotion: None,
        },
    }
}

/// ADR-011 item 2 rule 2/3: the status skeleton, refined by body text.
/// Ordering is the contract: validation text beats "5xx is retryable";
/// overload wording beats generic server error.
fn refine_by_body(status: u16, ev: &ErrorEvidence<'_>) -> Option<Classification> {
    if (500..600).contains(&status) {
        if let Some(m) = first_match(ev.body, FORMAT_ERROR_PATTERNS) {
            return Some(Classification {
                class: ErrorClass::FormatError,
                matched: m,
                retry_after: None,
                demotion: None,
            });
        }
        if let Some(m) = first_match(ev.body, OVERLOADED_PATTERNS) {
            return Some(Classification {
                class: ErrorClass::Overloaded,
                matched: m,
                retry_after: None,
                demotion: None,
            });
        }
        return None;
    }
    if status == 429 {
        if let Some(m) = first_match(ev.body, QUOTA_EXHAUSTED_PATTERNS) {
            return Some(Classification {
                class: ErrorClass::QuotaExhausted,
                matched: m,
                retry_after: None,
                demotion: None,
            });
        }
        // Confirm the rate-limit reading (the status already said it).
        if let Some(m) = first_match(ev.body, RATE_LIMIT_PATTERNS) {
            return Some(Classification {
                class: ErrorClass::RateLimit,
                matched: m,
                retry_after: None,
                demotion: None,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(status: Option<u16>, body: &str) -> ErrorEvidence<'_> {
        ErrorEvidence {
            status,
            retry_after: None,
            body: body.as_bytes(),
            wrote_full_request: false,
            transport_cause: None,
        }
    }

    #[test]
    fn status_skeleton() {
        assert_eq!(
            classify_upstream_error(&ev(Some(400), "")).class,
            ErrorClass::FormatError
        );
        assert_eq!(
            classify_upstream_error(&ev(Some(401), "")).class,
            ErrorClass::Auth
        );
        assert_eq!(
            classify_upstream_error(&ev(Some(404), "")).class,
            ErrorClass::ModelNotFound
        );
        assert_eq!(
            classify_upstream_error(&ev(Some(429), "")).class,
            ErrorClass::RateLimit
        );
        assert_eq!(
            classify_upstream_error(&ev(Some(503), "")).class,
            ErrorClass::ServerError
        );
        assert_eq!(
            classify_upstream_error(&ev(None, "")).class,
            ErrorClass::Timeout
        );
    }

    /// ADR-011 item 4: 403 + account-exhaustion wording demotes the provider.
    #[test]
    fn quota_exhaustion_demotes_provider() {
        let c = classify_upstream_error(&ev(
            Some(403),
            "{\"error\":{\"code\":\"insufficient_quota\"}}",
        ));
        assert_eq!(c.class, ErrorClass::QuotaExhausted);
        assert_eq!(c.matched, "insufficient_quota");
        assert!(c.class.demotes_provider());
        let d = c.demotion.expect("quota exhaustion orders a demotion");
        assert!(d.provider_wide);
        assert_eq!(d.ttl, DEFAULT_COOLDOWN);
    }

    #[test]
    fn retry_after_is_honored_as_state() {
        let e = ErrorEvidence {
            status: Some(429),
            retry_after: Some("30"),
            body: b"rate limit exceeded",
            wrote_full_request: false,
            transport_cause: None,
        };
        let c = classify_upstream_error(&e);
        assert_eq!(c.class, ErrorClass::RateLimit);
        assert_eq!(c.retry_after, Some(Duration::from_secs(30)));
        // Rate limit is a route fact, not a provider death (ADR-011 item 5).
        assert!(c.demotion.is_none());
    }

    /// ADR-011 item 2's ordering lesson: a 5xx with validation text is
    /// format_error and never retried.
    #[test]
    fn five_hundred_with_validation_text_is_format_error() {
        let c = classify_upstream_error(&ev(Some(500), "{\"error\":\"Unknown parameter: foo\"}"));
        assert_eq!(c.class, ErrorClass::FormatError);
        assert!(!c.class.fails_over());
    }

    #[test]
    fn overload_wording_beats_generic_server_error() {
        let c = classify_upstream_error(&ev(Some(529), "overloaded"));
        assert_eq!(c.class, ErrorClass::Overloaded);
        assert!(c.class.fails_over());
    }

    /// Rule 1: a content-policy refusal is deterministic whatever the
    /// status says — even a 5xx or a missing status.
    #[test]
    fn content_policy_beats_status() {
        let c = classify_upstream_error(&ev(Some(500), "content_policy_violation"));
        assert_eq!(c.class, ErrorClass::ContentPolicyBlocked);
        assert!(!c.class.fails_over());
    }

    /// ADR-011 item 10: a non-JSON, non-UTF-8 body still classifies.
    #[test]
    fn non_utf8_body_classifies_on_status() {
        let c = classify_upstream_error(&ErrorEvidence {
            status: Some(429),
            retry_after: None,
            body: &[0xff, 0xfe, 0x00],
            wrote_full_request: false,
            transport_cause: None,
        });
        assert_eq!(c.class, ErrorClass::RateLimit);
    }

    /// 429 + quota wording is the account death, not a throttle.
    #[test]
    fn four_twenty_nine_with_quota_wording_is_quota_exhausted() {
        let c = classify_upstream_error(&ev(Some(429), "You exceeded your usage limit"));
        assert_eq!(c.class, ErrorClass::QuotaExhausted);
        assert!(c.demotion.is_some());
    }

    #[test]
    fn retry_after_parsing() {
        assert_eq!(parse_retry_after("120"), Some(Duration::from_secs(120)));
        assert_eq!(parse_retry_after(" 30 "), Some(Duration::from_secs(30)));
        // HTTP-date forms are ignored by the pure classifier (no clock).
        assert_eq!(parse_retry_after("Wed, 21 Oct 2026 07:28:00 GMT"), None);
        assert_eq!(parse_retry_after("garbage"), None);
    }

    /// The no-status split: a transport cause of `Connect` is
    /// `connect_failure` and fails over; `Timeout` (and a caller with no
    /// transport evidence) keep the `timeout` verdict and its abort
    /// semantics — a connection failure must never wear the timeout's
    /// clothes.
    #[test]
    fn no_status_split_connect_vs_timeout() {
        let connect = ErrorEvidence {
            status: None,
            retry_after: None,
            body: b"",
            wrote_full_request: false,
            transport_cause: Some(TransportCause::Connect),
        };
        let c = classify_upstream_error(&connect);
        assert_eq!(c.class, ErrorClass::ConnectFailure);
        assert_eq!(c.matched, "no-status");
        assert!(c.class.fails_over(), "nothing was billed: fail over");
        assert!(!c.class.demotes_provider());

        for cause in [TransportCause::Timeout, TransportCause::Other] {
            let e = ErrorEvidence {
                status: None,
                retry_after: None,
                body: b"",
                wrote_full_request: false,
                transport_cause: Some(cause),
            };
            assert_eq!(
                classify_upstream_error(&e).class,
                ErrorClass::Timeout,
                "{cause:?} keeps the timeout verdict"
            );
        }

        // No transport evidence at all: the pre-split verdict stands.
        assert_eq!(
            classify_upstream_error(&ev(None, "")).class,
            ErrorClass::Timeout
        );
    }

    /// A class that fails over vs the deterministic three.
    #[test]
    fn fails_over_matrix() {
        for c in [
            ErrorClass::Auth,
            ErrorClass::RateLimit,
            ErrorClass::QuotaExhausted,
            ErrorClass::Overloaded,
            ErrorClass::ServerError,
            ErrorClass::ConnectFailure,
            ErrorClass::ModelNotFound,
        ] {
            assert!(c.fails_over(), "{} must fail over", c.as_str());
        }
        for c in [
            ErrorClass::Timeout,
            ErrorClass::FormatError,
            ErrorClass::ContentPolicyBlocked,
        ] {
            assert!(!c.fails_over(), "{} must not fail over", c.as_str());
        }
    }
}
