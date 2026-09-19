//! Unified error envelope (DESIGN §12.7, implementing spec §8).
//! All non-2xx responses and stub endpoints share one shape:
//! `{"error":{"type","message","request_id","details"?}}`.

use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidRequest,
    UnknownProvider,
    UnknownModel,
    AutoNotSupported,
    CapabilityUnsupported,
    CostCapExceeded,
    QuotaExceeded,
    StatefulUnsupported,
    UpstreamError,
    UpstreamTimeout,
    NotImplemented,
    Internal,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::UnknownProvider => "unknown_provider",
            Self::UnknownModel => "unknown_model",
            Self::AutoNotSupported => "auto_not_supported",
            Self::CapabilityUnsupported => "capability_unsupported",
            Self::CostCapExceeded => "cost_cap_exceeded",
            Self::QuotaExceeded => "quota_exceeded",
            Self::StatefulUnsupported => "stateful_unsupported",
            Self::UpstreamError => "upstream_error",
            Self::UpstreamTimeout => "upstream_timeout",
            Self::NotImplemented => "not_implemented",
            Self::Internal => "internal",
        }
    }

    /// `error.type` → HTTP status code (the DESIGN §12.7 table, one-to-one).
    pub const fn http_status(self) -> u16 {
        match self {
            Self::InvalidRequest
            | Self::AutoNotSupported
            | Self::CapabilityUnsupported
            | Self::StatefulUnsupported => 400,
            Self::UnknownProvider | Self::UnknownModel => 404,
            Self::CostCapExceeded => 403,
            Self::QuotaExceeded => 429,
            Self::UpstreamError => 502,
            Self::UpstreamTimeout => 504,
            Self::NotImplemented => 501,
            Self::Internal => 500,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Clone, Serialize)]
pub struct ErrorDetail {
    pub r#type: &'static str,
    pub message: String,
    pub request_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

impl ErrorBody {
    pub fn new(code: ErrorCode, message: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self {
            error: ErrorDetail {
                r#type: code.as_str(),
                message: message.into(),
                request_id: request_id.into(),
                details: None,
            },
        }
    }
}
