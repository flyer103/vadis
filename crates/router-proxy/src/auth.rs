//! The inbound token-auth boundary guard (spec §4.7, DESIGN §12.11 — the
//! landing). A guard in front of the three protocol routes reads one
//! header, compares it in constant time against a token the process read
//! once at startup from the environment, and either passes the untouched
//! request through or answers `401` and leaves one trace line.
//!
//! Pure: no HTTP types, no I/O — the shape `resolve_session_key`
//! (§12.10.5) already established for reading inbound headers, so the
//! guard unit-tests without a rig. The wiring (route layers, the 401
//! response, the startup resolution) is router-cli's.

use router_core::config::WireApi;
use router_core::cost::Nano;
use router_core::error::ErrorCode;
use router_core::trace::{
    CostRec, DecisionRec, DecisionRecord, IdentityRec, PrefixRec, ProtocolRec, ResultRec, StateRec,
    TraceError, TRACE_SCHEMA_VERSION,
};
use router_core::transform::TransformMode;

/// The token this process expects, resolved once at startup from the env
/// var that `server.auth_token_env` names. Constructed only when that key
/// is written (spec §4.7): no key ⇒ no gate is installed at all, which is
/// the strongest form of "behaves as before".
#[derive(Clone)]
pub struct AuthGate {
    token: String,
}

/// `Refused.header` is what goes into the error body's `details.header`
/// and the trace record: `Some("authorization")` / `Some("x-api-key")`
/// when that header was read and did not match, `None` when the request
/// presented neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthVerdict {
    Admitted,
    Refused { header: Option<&'static str> },
}

impl AuthGate {
    pub fn new(token: String) -> Self {
        Self { token }
    }

    /// `headers` are (lowercased name, value) pairs — the same shape the
    /// forwarding path takes, so the guard and the session key sources
    /// read headers the same way. A match on either accepted form
    /// admits; a malformed form offers no credential but still names its
    /// header in the refusal (the diagnostic, spec §4.7).
    pub fn admits(&self, headers: &[(String, String)]) -> AuthVerdict {
        let authorization = headers
            .iter()
            .find(|(k, _)| k == "authorization")
            .map(|(_, v)| v.as_str());
        let x_api_key = headers
            .iter()
            .find(|(k, _)| k == "x-api-key")
            .map(|(_, v)| v.as_str());
        // The diagnostic names the first of the two the request actually
        // carried, whether or not its form was usable.
        let carried = match (authorization, x_api_key) {
            (Some(_), _) => Some("authorization"),
            (None, Some(_)) => Some("x-api-key"),
            (None, None) => None,
        };
        // `Bearer <token>`: case-insensitive scheme, a single space, the
        // value trimmed of surrounding whitespace (§12.11).
        let bearer = authorization.and_then(|v| {
            let v = v.trim();
            let (scheme, cred) = v.split_once(' ')?;
            if !scheme.eq_ignore_ascii_case("bearer") {
                return None;
            }
            let cred = cred.trim();
            if cred.is_empty() {
                return None;
            }
            Some(cred)
        });
        let presented = bearer.or_else(|| x_api_key.map(|v| v.trim()).filter(|v| !v.is_empty()));
        match presented {
            Some(p) if constant_time_eq(p.as_bytes(), self.token.as_bytes()) => {
                AuthVerdict::Admitted
            }
            _ => AuthVerdict::Refused { header: carried },
        }
    }
}

/// Constant-time equality in the compared length (spec §4.7): no early
/// return on the first differing byte, no branch on secret bytes. The
/// lengths are compared first — the accepted, documented leak that
/// `subtle`'s own slice comparison has. Hand-rolled: `subtle` is not on
/// §12.1's allowlist and a byte loop does not justify widening it.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

/// The two frozen messages (§12.11): no implementer invents one, and
/// neither ever contains the expected token, any prefix of it, or the
/// presented value.
pub fn refused_message(verdict: &AuthVerdict) -> String {
    match verdict {
        AuthVerdict::Refused { header: None } => "inbound auth: no token presented (send it as 'Authorization: Bearer <token>' or 'x-api-key: <token>')".to_string(),
        AuthVerdict::Refused { header: Some(_) } => "inbound auth: the presented token does not match the value of the environment variable named by server.auth_token_env".to_string(),
        AuthVerdict::Admitted => unreachable!("no message for an admission"),
    }
}

/// The refused request's record (spec §6's pre-pipeline class, field by
/// field — the spec table is authoritative; this is where it is built).
/// `now_epoch_s` and `overhead_ms` are the guard's own clock reads; the
/// `ts` uses the record formatter this crate already owns (§12.6) rather
/// than a second copy of it. `config_digest` is the loaded
/// configuration's identity (spec §6's boundary table: the same value
/// every record of this process carries — a fact of the config, not of
/// the request), handed in by the caller from the trace writer this
/// record is written through.
pub fn refused_record(
    request_id: &str,
    proto_in: WireApi,
    verdict: &AuthVerdict,
    now_epoch_s: u64,
    overhead_ms: u32,
    config_digest: &str,
) -> DecisionRecord {
    let header = match verdict {
        AuthVerdict::Refused { header } => *header,
        AuthVerdict::Admitted => unreachable!("no record for an admission"),
    };
    DecisionRecord {
        schema_version: TRACE_SCHEMA_VERSION,
        ts: crate::accounting::rfc3339_millis(now_epoch_s),
        config_digest: config_digest.to_string(),
        identity: IdentityRec {
            request_id: request_id.to_string(),
            // No `request.received` row exists: the guard runs before
            // §4.5's row 1. `0` is never a real id (`events.event_id`
            // starts at 1) — the sentinel the pre-route path writes.
            event_id: 0,
            client: "other",
            session: None,
            thread_id: None,
            turn_index: 0,
        },
        protocol: ProtocolRec {
            protocol_in: proto_in.as_str().to_string(),
            // No route was selected.
            protocol_out: None,
            translated: false,
            lossy: Vec::new(),
        },
        decision: DecisionRec {
            provider: String::new(),
            model: String::new(),
            requested_model: None,
            selection_source: "explicit".to_string(),
            plugin_chain: Vec::new(),
            decision_ms: 0,
        },
        state: StateRec {
            stateful_inbound: false,
            sticky_hit: false,
            cache_control_breaks: 0,
        },
        prefix: PrefixRec {
            blocks: Vec::new(),
            continuity: None,
        },
        // A request refused by the inbound auth guard never entered the
        // pipeline (spec §6's pre-pipeline class): no step was even
        // planned, so the mode word is `passthrough` and nothing is
        // claimed (ADR-019's "refused before the transform chain ran").
        transform_mode: TransformMode::Passthrough,
        transforms: Vec::new(),
        usage: router_core::Usage::default(),
        usage_missing: true,
        cost: CostRec {
            input_miss: Nano(0),
            input_hit: Nano(0),
            cache_write: Nano(0),
            output: Nano(0),
            peak_applied_pct: 100,
            total: Nano(0),
            // §4.7's guard refuses before any route resolves, so no table
            // priced this record: all-zero amounts under the USD default
            // (spec §4.8 — the choice cannot mislead when nothing is priced).
            currency: router_core::Currency::Usd,
            quota_after: None,
        },
        result: ResultRec {
            status: ErrorCode::Unauthorized.http_status(),
            upstream_status: None,
            failover_from: None,
            plan_switch: None,
            overhead_ms,
            upstream_ms: None,
        },
        errors: vec![TraceError {
            kind: TraceError::kind_for_code(ErrorCode::Unauthorized).to_string(),
            message: refused_message(verdict),
            plugin: None,
            details: Some(serde_json::json!({ "header": header })),
        }],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn equal_tokens_admit_via_both_headers() {
        let g = AuthGate::new("tok-conf45-secret".into());
        assert_eq!(
            g.admits(&hdrs(&[("authorization", "Bearer tok-conf45-secret")])),
            AuthVerdict::Admitted
        );
        assert_eq!(
            g.admits(&hdrs(&[("x-api-key", "tok-conf45-secret")])),
            AuthVerdict::Admitted
        );
        // Either one is enough; both to the same value is the normal case.
        assert_eq!(
            g.admits(&hdrs(&[
                ("authorization", "Bearer tok-conf45-secret"),
                ("x-api-key", "tok-conf45-secret"),
            ])),
            AuthVerdict::Admitted
        );
    }

    #[test]
    fn last_byte_difference_is_refused() {
        // The case a "startsWith" bug passes and this catches.
        let g = AuthGate::new("tok-conf45-secret".into());
        assert_eq!(
            g.admits(&hdrs(&[("authorization", "Bearer tok-conf45-secreX")])),
            AuthVerdict::Refused {
                header: Some("authorization")
            }
        );
    }

    #[test]
    fn prefix_and_superset_are_refused() {
        let g = AuthGate::new("tok-conf45-secret".into());
        assert_eq!(
            g.admits(&hdrs(&[("x-api-key", "tok-conf45")])),
            AuthVerdict::Refused {
                header: Some("x-api-key")
            }
        );
        assert_eq!(
            g.admits(&hdrs(&[("x-api-key", "tok-conf45-secret-extra")])),
            AuthVerdict::Refused {
                header: Some("x-api-key")
            }
        );
    }

    #[test]
    fn empty_presented_token_is_refused_both_ways() {
        let g = AuthGate::new("tok-conf45-secret".into());
        assert_eq!(
            g.admits(&hdrs(&[("x-api-key", "")])),
            AuthVerdict::Refused {
                header: Some("x-api-key")
            }
        );
        // An empty expected token admits nothing either — an empty value
        // never even reaches a gate (the startup refusal, §12.11), but
        // the guard must not treat "" as a wildcard.
        let empty = AuthGate::new(String::new());
        assert_eq!(
            empty.admits(&hdrs(&[("x-api-key", "")])),
            AuthVerdict::Refused {
                header: Some("x-api-key")
            }
        );
        assert_eq!(
            empty.admits(&hdrs(&[("authorization", "Bearer ")])),
            AuthVerdict::Refused {
                header: Some("authorization")
            }
        );
    }

    #[test]
    fn malformed_authorization_names_itself() {
        let g = AuthGate::new("tok-conf45-secret".into());
        // No scheme at all.
        assert_eq!(
            g.admits(&hdrs(&[("authorization", "tok-conf45-secret")])),
            AuthVerdict::Refused {
                header: Some("authorization")
            }
        );
        // `Bearer` with no credential.
        assert_eq!(
            g.admits(&hdrs(&[("authorization", "Bearer ")])),
            AuthVerdict::Refused {
                header: Some("authorization")
            }
        );
        // A wrong scheme.
        assert_eq!(
            g.admits(&hdrs(&[("authorization", "Basic dG9r")])),
            AuthVerdict::Refused {
                header: Some("authorization")
            }
        );
        // The scheme is case-insensitive.
        assert_eq!(
            g.admits(&hdrs(&[("authorization", "bearer tok-conf45-secret")])),
            AuthVerdict::Admitted
        );
    }

    #[test]
    fn no_headers_means_header_none() {
        let g = AuthGate::new("tok-conf45-secret".into());
        assert_eq!(
            g.admits(&hdrs(&[("content-type", "application/json")])),
            AuthVerdict::Refused { header: None }
        );
    }

    #[test]
    fn messages_never_contain_a_token() {
        let g = AuthGate::new("tok-conf45-secret".into());
        let v = g.admits(&hdrs(&[]));
        let m = refused_message(&v);
        assert!(!m.contains("tok-conf45-secret"));
        let v = g.admits(&hdrs(&[("x-api-key", "wrong-value")]));
        let m = refused_message(&v);
        assert!(!m.contains("tok-conf45-secret"));
        assert!(!m.contains("wrong-value"));
    }

    #[test]
    fn refused_record_matches_the_spec6_table() {
        let g = AuthGate::new("tok-conf45-secret".into());
        let v = g.admits(&hdrs(&[("authorization", "Bearer nope")]));
        let rec = refused_record(
            "req-1",
            WireApi::Chat,
            &v,
            1_789_256_462,
            3,
            "0123456789abcdef",
        );
        let j = serde_json::to_value(&rec).unwrap();
        assert_eq!(j["schema_version"], 2); // v2: cost.currency (ADR-018)
        assert_eq!(j["config_digest"], "0123456789abcdef"); // additive, version unmoved (ADR-037 D6)
        assert_eq!(j["cost"]["currency"], "USD");
        assert_eq!(j["ts"], "2026-09-12T23:41:02.000Z");
        assert_eq!(j["identity"]["request_id"], "req-1");
        assert_eq!(j["identity"]["event_id"], 0);
        assert_eq!(j["identity"]["session"], serde_json::Value::Null);
        assert_eq!(j["identity"]["turn_index"], 0);
        assert_eq!(j["protocol"]["protocol_in"], "chat");
        assert_eq!(j["protocol"]["protocol_out"], serde_json::Value::Null);
        assert_eq!(j["protocol"]["translated"], false);
        assert_eq!(j["decision"]["provider"], "");
        assert_eq!(j["decision"]["model"], "");
        assert_eq!(j["decision"]["requested_model"], serde_json::Value::Null);
        assert_eq!(j["decision"]["selection_source"], "explicit");
        assert_eq!(j["decision"]["decision_ms"], 0);
        assert_eq!(j["state"]["stateful_inbound"], false);
        assert_eq!(j["prefix"]["blocks"], serde_json::json!([]));
        assert_eq!(j["prefix"]["continuity"], serde_json::Value::Null);
        assert!(j.get("transforms").is_none(), "omitted when empty");
        assert_eq!(j["usage"]["input_total"], 0);
        assert_eq!(j["usage_missing"], true);
        assert_eq!(j["cost"]["input_miss"], 0);
        assert_eq!(j["cost"]["input_hit"], 0);
        assert_eq!(j["cost"]["cache_write"], 0);
        assert_eq!(j["cost"]["output"], 0);
        assert_eq!(j["cost"]["total"], 0);
        assert_eq!(j["cost"]["quota_after"], serde_json::Value::Null);
        assert_eq!(j["result"]["status"], 401);
        assert_eq!(j["result"]["upstream_status"], serde_json::Value::Null);
        assert_eq!(j["result"]["failover_from"], serde_json::Value::Null);
        assert_eq!(j["result"]["plan_switch"], serde_json::Value::Null);
        assert_eq!(j["result"]["overhead_ms"], 3);
        assert_eq!(j["result"]["upstream_ms"], serde_json::Value::Null);
        let errors = j["errors"].as_array().unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0]["kind"], "unauthorized");
        assert_eq!(errors[0]["details"]["header"], "authorization");
    }

    #[test]
    fn refused_record_without_a_header_carries_null() {
        let g = AuthGate::new("tok-conf45-secret".into());
        let v = g.admits(&hdrs(&[]));
        let rec = refused_record("req-2", WireApi::Anthropic, &v, 0, 0, "");
        let j = serde_json::to_value(&rec).unwrap();
        assert_eq!(j["errors"][0]["details"]["header"], serde_json::Value::Null);
        assert_eq!(j["protocol"]["protocol_in"], "anthropic");
    }
}
