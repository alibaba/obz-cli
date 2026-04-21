//! Error types for the obz response envelope.
//!
//! Each error carries a category, code, human-readable message, and
//! optional fields for provider context, raw backend response,
//! recoverability flag, suggested fix, documentation URL, and source
//! chain. This enables AI Agents to programmatically classify errors,
//! decide whether to retry, and take corrective action.

use serde::{Deserialize, Serialize};

/// Structured error detail for the response envelope.
///
/// Provides machine-readable category/code, human-readable message,
/// provider context, recoverability flag, suggested fix, and an
/// optional source chain for debugging.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ErrorDetail {
    /// Error category for broad classification.
    pub category: ErrorCategory,

    /// Machine-readable error code.
    pub code: ErrorCode,

    /// The provider that triggered the error (if applicable).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,

    /// Human-readable error description.
    pub message: String,

    /// Raw error from the backend API (for debugging).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_error: Option<String>,

    /// Whether the error is recoverable (retry or adjust parameters).
    pub recoverable: bool,

    /// Suggested fix for the error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggestion: Option<String>,

    /// Documentation URL for more information.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub doc_url: Option<String>,

    /// Error source chain for debugging (e.g. underlying TLS/DNS/IO errors).
    ///
    /// Each entry is the `Display` output of a successive `.source()` in the
    /// original error chain.  Only populated when the error wraps an external
    /// library error (e.g. `reqwest`, `serde_json`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_chain: Option<Vec<String>>,
}

/// Error category for broad classification.
///
/// Maps to exit codes: auth=1, flag=2, provider=3, network=4, unsupported=5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    /// Authentication/authorization error (exit code 1).
    Auth,
    /// Invalid CLI arguments or flags (exit code 2).
    Flag,
    /// Backend provider error (4xx/5xx) (exit code 3).
    Provider,
    /// Network error (DNS, timeout, TLS) (exit code 4).
    Network,
    /// Operation not supported by this provider (exit code 5).
    Unsupported,
}

impl ErrorCategory {
    /// Return the process exit code for this error category.
    pub fn exit_code(self) -> i32 {
        match self {
            Self::Auth => 1,
            Self::Flag => 2,
            Self::Provider => 3,
            Self::Network => 4,
            Self::Unsupported => 5,
        }
    }
}

/// Machine-readable error code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    // Auth errors
    /// Authentication credentials are missing.
    AuthMissing,
    /// Authentication credentials have expired.
    AuthExpired,
    /// Access denied (insufficient permissions).
    AccessDenied,

    // Flag errors
    /// Invalid query syntax.
    QuerySyntax,
    /// Invalid time range or format.
    InvalidTimeRange,
    /// Missing required flag.
    MissingRequired,
    /// Invalid flag value.
    InvalidFlag,
    /// Configuration file read or parse error.
    ConfigError,

    // Provider errors
    /// Backend returned an error response.
    BackendError,
    /// Rate limited by the backend.
    RateLimited,
    /// Requested resource not found.
    NotFound,
    /// Request timed out.
    Timeout,

    // Network errors
    /// DNS resolution failed.
    DnsError,
    /// TLS/SSL handshake failed.
    TlsError,
    /// Connection refused or reset.
    ConnectionError,

    // Unsupported
    /// This operation is not supported by the provider.
    NotSupported,
}

/// The core error type for obz-core operations.
///
/// Uses `thiserror` for ergonomic error handling. Each variant carries
/// enough information to produce a structured [`ErrorDetail`] for the
/// response envelope.
#[derive(Debug, thiserror::Error)]
pub enum ObzError {
    /// Provider returned an error.
    #[error("{message}")]
    Provider {
        /// Machine-readable error code.
        code: ErrorCode,
        /// Human-readable message.
        message: String,
        /// Raw backend error response.
        raw_error: Option<String>,
        /// Whether a retry might succeed.
        recoverable: bool,
        /// Suggested fix.
        suggestion: Option<String>,
        /// Documentation URL.
        doc_url: Option<String>,
    },

    /// Authentication error.
    #[error("authentication error: {message}")]
    Auth {
        /// Machine-readable error code.
        code: ErrorCode,
        /// Human-readable message.
        message: String,
        /// Whether the caller can recover by retrying the same request.
        ///
        /// Set `true` **only** when the underlying cause has already been
        /// resolved (e.g. credential-process refreshed expired credentials).
        /// All other auth errors should use `false` — downstream consumers
        /// (AI Agents) may auto-retry based on this flag.
        recoverable: bool,
        /// Suggested fix.
        suggestion: Option<String>,
    },

    /// Invalid CLI arguments.
    #[error("invalid argument: {message}")]
    InvalidArgument {
        /// Machine-readable error code.
        code: ErrorCode,
        /// Human-readable message.
        message: String,
        /// Suggested fix.
        suggestion: Option<String>,
    },

    /// Network error.
    #[error("network error: {message}")]
    Network {
        /// Machine-readable error code.
        code: ErrorCode,
        /// Human-readable message.
        message: String,
        /// Whether a retry might succeed.
        recoverable: bool,
        /// Error source chain from the underlying library error.
        source_chain: Option<Vec<String>>,
    },

    /// Operation not supported by this provider.
    #[error("{message}")]
    Unsupported {
        /// Human-readable message.
        message: String,
        /// The provider that doesn't support this operation.
        provider: Option<String>,
        /// Suggested fix.
        suggestion: Option<String>,
    },
}

impl ObzError {
    /// Convert this error into a structured [`ErrorDetail`] for JSON output.
    ///
    /// `provider` is injected into `ErrorDetail.provider` only when the
    /// variant does not already carry one (e.g. [`Unsupported`] sets its
    /// own provider, which takes precedence over the caller-supplied value).
    pub fn to_error_detail(&self, provider: Option<&str>) -> ErrorDetail {
        let mut detail = match self {
            Self::Provider {
                code,
                message,
                raw_error,
                recoverable,
                suggestion,
                doc_url,
            } => ErrorDetail {
                category: ErrorCategory::Provider,
                code: *code,
                provider: None,
                message: message.clone(),
                raw_error: raw_error.clone(),
                recoverable: *recoverable,
                suggestion: suggestion.clone(),
                doc_url: doc_url.clone(),
                source_chain: None,
            },
            Self::Auth {
                code,
                message,
                recoverable,
                suggestion,
            } => ErrorDetail {
                category: ErrorCategory::Auth,
                code: *code,
                provider: None,
                message: message.clone(),
                raw_error: None,
                recoverable: *recoverable,
                suggestion: suggestion.clone(),
                doc_url: None,
                source_chain: None,
            },
            Self::InvalidArgument {
                code,
                message,
                suggestion,
            } => ErrorDetail {
                category: ErrorCategory::Flag,
                code: *code,
                provider: None,
                message: message.clone(),
                raw_error: None,
                recoverable: false,
                suggestion: suggestion.clone(),
                doc_url: None,
                source_chain: None,
            },
            Self::Network {
                code,
                message,
                recoverable,
                source_chain,
            } => ErrorDetail {
                category: ErrorCategory::Network,
                code: *code,
                provider: None,
                message: message.clone(),
                raw_error: None,
                recoverable: *recoverable,
                suggestion: None,
                doc_url: None,
                source_chain: source_chain.clone(),
            },
            Self::Unsupported {
                message,
                provider,
                suggestion,
            } => ErrorDetail {
                category: ErrorCategory::Unsupported,
                code: ErrorCode::NotSupported,
                provider: provider.clone(),
                message: message.clone(),
                raw_error: None,
                recoverable: false,
                suggestion: suggestion.clone(),
                doc_url: None,
                source_chain: None,
            },
        };

        // Inject provider from caller context if variant didn't set one.
        if detail.provider.is_none() {
            detail.provider = provider.map(str::to_string);
        }

        detail
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_category_exit_codes() {
        assert_eq!(ErrorCategory::Auth.exit_code(), 1);
        assert_eq!(ErrorCategory::Flag.exit_code(), 2);
        assert_eq!(ErrorCategory::Provider.exit_code(), 3);
        assert_eq!(ErrorCategory::Network.exit_code(), 4);
        assert_eq!(ErrorCategory::Unsupported.exit_code(), 5);
    }

    #[test]
    fn test_error_category_serialization() {
        assert_eq!(
            serde_json::to_string(&ErrorCategory::Auth).unwrap(),
            r#""auth""#
        );
        assert_eq!(
            serde_json::to_string(&ErrorCategory::Provider).unwrap(),
            r#""provider""#
        );
        assert_eq!(
            serde_json::to_string(&ErrorCategory::Unsupported).unwrap(),
            r#""unsupported""#
        );
    }

    #[test]
    fn test_error_code_config_error_serialization() {
        assert_eq!(
            serde_json::to_string(&ErrorCode::ConfigError).unwrap(),
            r#""config_error""#
        );
    }

    #[test]
    fn test_config_error_to_detail() {
        let err = ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: "failed to parse config.yaml".to_string(),
            suggestion: None,
        };
        let detail = err.to_error_detail(None);
        assert_eq!(detail.category, ErrorCategory::Flag);
        assert_eq!(detail.code, ErrorCode::ConfigError);
        assert!(!detail.recoverable);
        assert!(detail.source_chain.is_none());
    }

    #[test]
    fn test_obz_error_to_detail() {
        let err = ObzError::Provider {
            code: ErrorCode::QuerySyntax,
            message: "invalid expression".to_string(),
            raw_error: Some("bad_data".to_string()),
            recoverable: false,
            suggestion: Some("Check your PromQL syntax".to_string()),
            doc_url: None,
        };

        let detail = err.to_error_detail(None);
        assert_eq!(detail.category, ErrorCategory::Provider);
        assert_eq!(detail.code, ErrorCode::QuerySyntax);
        assert!(!detail.recoverable);
    }

    #[test]
    fn test_network_error_preserves_source_chain() {
        let err = ObzError::Network {
            code: ErrorCode::TlsError,
            message: "TLS error: certificate verify failed".to_string(),
            recoverable: false,
            source_chain: Some(vec![
                "rustls::Error::InvalidCertificate(UnknownIssuer)".to_string(),
                "certificate not trusted: CA not in trust store".to_string(),
            ]),
        };
        let detail = err.to_error_detail(None);
        assert_eq!(detail.category, ErrorCategory::Network);
        assert_eq!(detail.code, ErrorCode::TlsError);
        let chain = detail.source_chain.unwrap();
        assert_eq!(chain.len(), 2);
        assert!(chain[0].contains("UnknownIssuer"));
    }

    #[test]
    fn test_source_chain_none_omitted_from_json() {
        let detail = ErrorDetail {
            category: ErrorCategory::Flag,
            code: ErrorCode::MissingRequired,
            provider: None,
            message: "missing --provider".to_string(),
            raw_error: None,
            recoverable: false,
            suggestion: None,
            doc_url: None,
            source_chain: None,
        };
        let json = serde_json::to_string(&detail).unwrap();
        assert!(
            !json.contains("source_chain"),
            "None source_chain should be omitted: {json}"
        );
    }

    #[test]
    fn test_auth_recoverable_true_propagated() {
        let err = ObzError::Auth {
            code: ErrorCode::AuthExpired,
            message: "token expired".to_string(),
            recoverable: true,
            suggestion: Some("Retry the command".to_string()),
        };
        let detail = err.to_error_detail(None);
        assert!(detail.recoverable);
        assert_eq!(detail.suggestion.as_deref(), Some("Retry the command"));
    }

    #[test]
    fn test_auth_recoverable_false_propagated() {
        let err = ObzError::Auth {
            code: ErrorCode::AuthMissing,
            message: "no credentials".to_string(),
            recoverable: false,
            suggestion: None,
        };
        let detail = err.to_error_detail(None);
        assert!(!detail.recoverable);
    }

    #[test]
    fn test_invalid_argument_suggestion_propagated() {
        let err = ObzError::InvalidArgument {
            code: ErrorCode::MissingRequired,
            message: "--provider is required".to_string(),
            suggestion: Some("Set default_provider in config.yaml".to_string()),
        };
        let detail = err.to_error_detail(None);
        assert_eq!(
            detail.suggestion.as_deref(),
            Some("Set default_provider in config.yaml")
        );
    }

    #[test]
    fn test_invalid_argument_suggestion_none() {
        let err = ObzError::InvalidArgument {
            code: ErrorCode::InvalidTimeRange,
            message: "invalid time".to_string(),
            suggestion: None,
        };
        let detail = err.to_error_detail(None);
        assert!(detail.suggestion.is_none());
    }

    #[test]
    fn test_to_error_detail_injects_provider() {
        let err = ObzError::Provider {
            code: ErrorCode::BackendError,
            message: "HTTP 500".to_string(),
            raw_error: None,
            recoverable: true,
            suggestion: None,
            doc_url: None,
        };
        let detail = err.to_error_detail(Some("my-vm"));
        assert_eq!(detail.provider.as_deref(), Some("my-vm"));
    }

    #[test]
    fn test_to_error_detail_does_not_override_existing_provider() {
        let err = ObzError::Unsupported {
            message: "not supported".to_string(),
            provider: Some("existing-provider".to_string()),
            suggestion: None,
        };
        let detail = err.to_error_detail(Some("caller-provider"));
        assert_eq!(detail.provider.as_deref(), Some("existing-provider"));
    }

    #[test]
    fn test_source_chain_some_included_in_json() {
        let detail = ErrorDetail {
            category: ErrorCategory::Network,
            code: ErrorCode::TlsError,
            provider: None,
            message: "TLS error".to_string(),
            raw_error: None,
            recoverable: false,
            suggestion: None,
            doc_url: None,
            source_chain: Some(vec!["cause1".to_string(), "cause2".to_string()]),
        };
        let json = serde_json::to_value(&detail).unwrap();
        let chain = json["source_chain"].as_array().unwrap();
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0], "cause1");
        assert_eq!(chain[1], "cause2");
    }
}
