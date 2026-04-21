//! Provider interface definitions.
//!
//! This module defines the contracts between the CLI and backend providers:
//!
//! - [`traits`] — `MetricProvider`, `LogProvider`, `TraceProvider` trait definitions
//! - [`params`] — Query parameter types passed to provider methods
//! - [`results`] — Result types returned by provider methods
//! - [`ProviderConfig`] — Dynamic key-value configuration passed to provider factories
//!
//! Concrete provider implementations live in the `obz-providers` crate.

pub mod params;
pub mod results;
pub mod traits;

use std::collections::BTreeMap;
use std::time::Duration;

use crate::model::error::{ErrorCode, ObzError};

// Re-export commonly used items at the module level.
pub use params::{
    ExtensionParams, LabelValuesParams, LogSearchParams, MetricInfoParams, MetricMetadataParams,
    MetricQueryParams, TraceGetParams, TraceSearchParams,
};
pub use results::{
    ExtensionResult, LogSearchResult, MetricQueryResult, MetricResultType, ProviderResult,
    TraceSearchResult,
};
pub use traits::{ExtensionProvider, LogProvider, MetricProvider, TraceProvider};

/// Dynamic configuration values resolved from config file profiles and
/// CLI flags, passed to provider factory functions.
///
/// Configuration is organized into four namespaces:
///
/// - **values** — Connection parameters and provider-specific settings
///   (endpoint, project, logstore, index, …).
/// - **auth** — Authentication credentials (token, username, password,
///   access-key-id, api-key, …). Empty-string values are treated as
///   absent.
/// - **headers** — Custom HTTP headers injected into every request.
///   Keys are stored in lowercase.
/// - **verbose** / **timeout** — Promoted from string-key conventions
///   to typed fields.
///
/// # Security
///
/// [`Debug`] is manually implemented:
/// - The entire `auth` map is printed as `[REDACTED]`.
/// - Header values containing sensitive substrings (`token`, `secret`,
///   `key`, `auth`) are individually redacted.
/// - `values` uses [`is_sensitive_key`] for per-key redaction.
///
/// # Example
///
/// ```
/// use obz_core::ProviderConfig;
///
/// let mut config = ProviderConfig::new();
/// config.set("endpoint", "http://localhost:8428");
/// config.set_auth("token", "my-secret");
///
/// assert_eq!(config.get("endpoint"), Some("http://localhost:8428"));
/// assert_eq!(config.bearer_token(), Some("my-secret".to_string()));
/// assert!(config.require("endpoint").is_ok());
/// assert!(config.require("missing").is_err());
///
/// // Debug output redacts auth entirely
/// let debug = format!("{:?}", config);
/// assert!(debug.contains("[REDACTED]"));
/// assert!(!debug.contains("my-secret"));
/// ```
#[derive(Clone)]
pub struct ProviderConfig {
    /// Connection parameters and provider-specific settings.
    values: BTreeMap<String, String>,
    /// Authentication credentials. Empty-string values are treated as absent.
    auth: BTreeMap<String, String>,
    /// Custom HTTP headers (keys stored in lowercase).
    headers: BTreeMap<String, String>,
    /// Whether verbose/debug output is enabled.
    verbose: bool,
    /// HTTP client timeout.
    timeout: Option<Duration>,
}

impl ProviderConfig {
    /// Create an empty configuration.
    pub fn new() -> Self {
        Self {
            values: BTreeMap::new(),
            auth: BTreeMap::new(),
            headers: BTreeMap::new(),
            verbose: false,
            timeout: None,
        }
    }

    // ── Values (connection parameters) ──────────────────────

    /// Set a configuration value, returning `&mut Self` for chaining.
    ///
    /// If the key already exists, its value is overwritten.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> &mut Self {
        self.values.insert(key.to_string(), value.into());
        self
    }

    /// Get a configuration value by key.
    ///
    /// Returns `None` if the key is not present.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// Get a cloned configuration value by key.
    ///
    /// Convenience method for cases where an owned `String` is needed
    /// (e.g., passing to a constructor that takes `Option<String>`).
    pub fn get_owned(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }

    /// Get a required configuration value by key.
    ///
    /// # Errors
    ///
    /// Returns [`ObzError::InvalidArgument`] with [`ErrorCode::MissingRequired`]
    /// if the key is not present. The error message suggests setting the value
    /// in `config.yaml`.
    pub fn require(&self, key: &str) -> Result<&str, ObzError> {
        self.get(key).ok_or_else(|| ObzError::InvalidArgument {
            code: ErrorCode::MissingRequired,
            message: format!("--{key} is required"),
            suggestion: None,
        })
    }

    /// Get a required provider configuration value by key.
    ///
    /// Like [`require`](Self::require), but the error message includes a
    /// hint pointing the user to `config.yaml`. Use this for provider
    /// config fields (e.g. `endpoint`) that are typically set in the
    /// config file rather than on the command line.
    ///
    /// # Errors
    ///
    /// Returns [`ObzError::InvalidArgument`] with [`ErrorCode::MissingRequired`]
    /// if the key is not present.
    pub fn require_config(&self, key: &str) -> Result<&str, ObzError> {
        self.get(key).ok_or_else(|| ObzError::InvalidArgument {
            code: ErrorCode::MissingRequired,
            message: format!(
                "--{key} is required. Set it in config.yaml under your provider's config block"
            ),
            suggestion: None,
        })
    }

    // ── Auth (credentials) ──────────────────────────────────

    /// Set an auth credential value.
    ///
    /// Empty strings are stored but [`auth_get`](Self::auth_get) treats
    /// them as absent.
    pub fn set_auth(&mut self, key: &str, value: impl Into<String>) -> &mut Self {
        self.auth.insert(key.to_string(), value.into());
        self
    }

    /// Get an auth credential value by key.
    ///
    /// Returns `None` if the key is missing **or** its value is an empty
    /// string (empty auth values are treated as absent to prevent
    /// accidental empty-credential requests).
    pub fn auth_get(&self, key: &str) -> Option<&str> {
        self.auth
            .get(key)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }

    /// Get a cloned auth credential value by key.
    ///
    /// Returns `None` if the key is missing or empty.
    pub fn auth_get_owned(&self, key: &str) -> Option<String> {
        self.auth_get(key).map(str::to_string)
    }

    /// Get a required auth credential value by key.
    ///
    /// # Errors
    ///
    /// Returns [`ObzError::InvalidArgument`] with [`ErrorCode::MissingRequired`]
    /// if the key is missing or empty. The error message directs the user to
    /// set the value in `config.yaml` under `providers.<name>.auth.<key>`.
    pub fn auth_require(&self, key: &str) -> Result<&str, ObzError> {
        self.auth_get(key)
            .ok_or_else(|| auth_missing_error(key, ""))
    }

    /// Get the bearer token from auth credentials.
    ///
    /// Shorthand for `self.auth_get_owned("token")`.
    pub fn bearer_token(&self) -> Option<String> {
        self.auth_get_owned("token")
    }

    /// Extract HTTP Basic Auth credentials (`username`, `password`) from
    /// the auth map.
    ///
    /// Returns `None` when either key is missing or empty, which means
    /// the provider should skip basic-auth and fall through to other auth
    /// methods (e.g., bearer token).
    pub fn basic_auth(&self) -> Option<(String, String)> {
        match (self.auth_get("username"), self.auth_get("password")) {
            (Some(u), Some(p)) => Some((u.to_string(), p.to_string())),
            _ => None,
        }
    }

    // ── Headers ─────────────────────────────────────────────

    /// Set a custom HTTP header. The key is automatically lowercased.
    pub fn set_header(&mut self, key: &str, value: impl Into<String>) -> &mut Self {
        self.headers.insert(key.to_ascii_lowercase(), value.into());
        self
    }

    /// Return the custom headers map.
    pub fn custom_headers(&self) -> &BTreeMap<String, String> {
        &self.headers
    }

    // ── Verbose / Timeout ───────────────────────────────────

    /// Set the verbose flag.
    pub fn set_verbose(&mut self, verbose: bool) {
        self.verbose = verbose;
    }

    /// Check whether verbose/debug output is enabled.
    pub fn verbose(&self) -> bool {
        self.verbose
    }

    /// Set the HTTP client timeout.
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = Some(timeout);
    }

    /// Get the HTTP client timeout.
    pub fn timeout(&self) -> Option<Duration> {
        self.timeout
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` if a key name likely holds a sensitive value.
///
/// Matches keys containing `token`, `password`, `secret` as substrings,
/// and keys ending with `-key` or equal to `key` (to avoid false positives
/// on keys like `"monkey"` or `"hotkey"`).
pub fn is_sensitive_key(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    k.contains("token")
        || k.contains("password")
        || k.contains("secret")
        || k == "key"
        || k.ends_with("-key")
}

/// Create a standardized error for missing auth credentials.
///
/// Produces a consistent error message directing the user to set the
/// credential in `config.yaml` under the provider's `auth` section.
pub fn auth_missing_error(key: &str, provider_type: &str) -> ObzError {
    let hint = if provider_type.is_empty() {
        format!("Set it in config.yaml: providers.<name>.auth.{key}")
    } else {
        format!(
            "Set it in config.yaml: providers.<name>.auth.{key} (provider type: {provider_type})"
        )
    };
    ObzError::InvalidArgument {
        code: ErrorCode::MissingRequired,
        message: format!("{key} is required. {hint}"),
        suggestion: None,
    }
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut s = f.debug_struct("ProviderConfig");

        // Values: per-key redaction using is_sensitive_key.
        let redacted_values: BTreeMap<&str, &str> = self
            .values
            .iter()
            .map(|(k, v)| {
                if is_sensitive_key(k) {
                    (k.as_str(), "[REDACTED]")
                } else {
                    (k.as_str(), v.as_str())
                }
            })
            .collect();
        s.field("values", &redacted_values);

        // Auth: entire map is redacted.
        if self.auth.is_empty() {
            s.field("auth", &"{}");
        } else {
            s.field("auth", &"[REDACTED]");
        }

        // Headers: selective redaction.
        let redacted_headers: BTreeMap<&str, &str> = self
            .headers
            .iter()
            .map(|(k, v)| {
                if is_sensitive_key(k) {
                    (k.as_str(), "[REDACTED]")
                } else {
                    (k.as_str(), v.as_str())
                }
            })
            .collect();
        s.field("headers", &redacted_headers);

        s.field("verbose", &self.verbose);
        s.field("timeout", &self.timeout);
        s.finish()
    }
}

/// Observability signal type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Metrics signal.
    Metric,
    /// Logs signal.
    Log,
    /// Traces signal.
    Trace,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Values tests ───────────────────────────────────────

    #[test]
    fn test_get_returns_value_when_key_exists() {
        let mut config = ProviderConfig::new();
        config.set("endpoint", "http://localhost:8428");
        assert_eq!(config.get("endpoint"), Some("http://localhost:8428"));
    }

    #[test]
    fn test_get_returns_none_when_key_missing() {
        let config = ProviderConfig::new();
        assert_eq!(config.get("endpoint"), None);
    }

    #[test]
    fn test_get_owned_returns_cloned_value() {
        let mut config = ProviderConfig::new();
        config.set("project", "abc");
        assert_eq!(config.get_owned("project"), Some("abc".to_string()));
        assert_eq!(config.get_owned("missing"), None);
    }

    #[test]
    fn test_require_returns_value_when_key_exists() {
        let mut config = ProviderConfig::new();
        config.set("endpoint", "http://localhost:8428");
        assert_eq!(config.require("endpoint").unwrap(), "http://localhost:8428");
    }

    #[test]
    fn test_require_returns_error_when_key_missing() {
        let config = ProviderConfig::new();
        let err = config.require("endpoint").unwrap_err();
        match err {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--endpoint"));
            }
            _ => panic!("expected InvalidArgument, got {err:?}"),
        }
    }

    #[test]
    fn test_require_config_returns_value_when_key_exists() {
        let mut config = ProviderConfig::new();
        config.set("endpoint", "http://localhost:8428");
        assert_eq!(
            config.require_config("endpoint").unwrap(),
            "http://localhost:8428"
        );
    }

    #[test]
    fn test_require_config_error_includes_config_hint() {
        let config = ProviderConfig::new();
        let err = config.require_config("endpoint").unwrap_err();
        match err {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("--endpoint"));
                assert!(
                    message.contains("config.yaml"),
                    "require_config error should mention config.yaml, got: {message}"
                );
            }
            _ => panic!("expected InvalidArgument, got {err:?}"),
        }
    }

    #[test]
    fn test_require_error_does_not_mention_config() {
        let config = ProviderConfig::new();
        let err = config.require("query").unwrap_err();
        match err {
            ObzError::InvalidArgument { message, .. } => {
                assert!(
                    !message.contains("config.yaml"),
                    "require() should not mention config.yaml, got: {message}"
                );
            }
            _ => panic!("expected InvalidArgument, got {err:?}"),
        }
    }

    #[test]
    fn test_set_overwrites_existing_value() {
        let mut config = ProviderConfig::new();
        config.set("endpoint", "http://old");
        config.set("endpoint", "http://new");
        assert_eq!(config.get("endpoint"), Some("http://new"));
    }

    #[test]
    fn test_set_supports_chaining() {
        let mut config = ProviderConfig::new();
        config
            .set("endpoint", "http://localhost")
            .set("project", "abc");
        assert_eq!(config.get("endpoint"), Some("http://localhost"));
        assert_eq!(config.get("project"), Some("abc"));
    }

    // ── Auth tests ─────────────────────────────────────────

    #[test]
    fn test_auth_get_returns_value() {
        let mut config = ProviderConfig::new();
        config.set_auth("token", "my-token");
        assert_eq!(config.auth_get("token"), Some("my-token"));
    }

    #[test]
    fn test_auth_get_returns_none_when_missing() {
        let config = ProviderConfig::new();
        assert_eq!(config.auth_get("token"), None);
    }

    #[test]
    fn test_auth_get_returns_none_for_empty_string() {
        let mut config = ProviderConfig::new();
        config.set_auth("token", "");
        assert_eq!(config.auth_get("token"), None);
    }

    #[test]
    fn test_auth_get_owned_returns_cloned_value() {
        let mut config = ProviderConfig::new();
        config.set_auth("api-key", "abc123");
        assert_eq!(config.auth_get_owned("api-key"), Some("abc123".to_string()));
        assert_eq!(config.auth_get_owned("missing"), None);
    }

    #[test]
    fn test_auth_require_returns_value() {
        let mut config = ProviderConfig::new();
        config.set_auth("token", "secret");
        assert_eq!(config.auth_require("token").unwrap(), "secret");
    }

    #[test]
    fn test_auth_require_returns_error_when_missing() {
        let config = ProviderConfig::new();
        let err = config.auth_require("token").unwrap_err();
        match err {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("token"));
                assert!(message.contains("config.yaml"));
            }
            _ => panic!("expected InvalidArgument, got {err:?}"),
        }
    }

    #[test]
    fn test_auth_require_returns_error_for_empty_string() {
        let mut config = ProviderConfig::new();
        config.set_auth("token", "");
        assert!(config.auth_require("token").is_err());
    }

    #[test]
    fn test_bearer_token_returns_token_from_auth() {
        let mut config = ProviderConfig::new();
        config.set_auth("token", "bearer-secret");
        assert_eq!(config.bearer_token(), Some("bearer-secret".to_string()));
    }

    #[test]
    fn test_bearer_token_returns_none_when_missing() {
        let config = ProviderConfig::new();
        assert_eq!(config.bearer_token(), None);
    }

    #[test]
    fn test_basic_auth_returns_credentials_when_both_present() {
        let mut config = ProviderConfig::new();
        config.set_auth("username", "admin");
        config.set_auth("password", "secret");
        assert_eq!(
            config.basic_auth(),
            Some(("admin".to_string(), "secret".to_string()))
        );
    }

    #[test]
    fn test_basic_auth_returns_none_when_username_missing() {
        let mut config = ProviderConfig::new();
        config.set_auth("password", "secret");
        assert_eq!(config.basic_auth(), None);
    }

    #[test]
    fn test_basic_auth_returns_none_when_password_missing() {
        let mut config = ProviderConfig::new();
        config.set_auth("username", "admin");
        assert_eq!(config.basic_auth(), None);
    }

    #[test]
    fn test_basic_auth_returns_none_when_both_missing() {
        let config = ProviderConfig::new();
        assert_eq!(config.basic_auth(), None);
    }

    #[test]
    fn test_basic_auth_returns_none_when_empty_strings() {
        let mut config = ProviderConfig::new();
        config.set_auth("username", "");
        config.set_auth("password", "secret");
        assert_eq!(config.basic_auth(), None);
    }

    // ── Headers tests ──────────────────────────────────────

    #[test]
    fn test_set_header_lowercases_key() {
        let mut config = ProviderConfig::new();
        config.set_header("X-Scope-OrgID", "my-tenant");
        assert_eq!(
            config.custom_headers().get("x-scope-orgid"),
            Some(&"my-tenant".to_string())
        );
    }

    #[test]
    fn test_custom_headers_returns_all_headers() {
        let mut config = ProviderConfig::new();
        config.set_header("x-custom", "value1");
        config.set_header("x-other", "value2");
        assert_eq!(config.custom_headers().len(), 2);
    }

    // ── Verbose / Timeout tests ────────────────────────────

    #[test]
    fn test_verbose_returns_true_when_set() {
        let mut config = ProviderConfig::new();
        config.set_verbose(true);
        assert!(config.verbose());
    }

    #[test]
    fn test_verbose_returns_false_by_default() {
        let config = ProviderConfig::new();
        assert!(!config.verbose());
    }

    #[test]
    fn test_timeout_returns_none_by_default() {
        let config = ProviderConfig::new();
        assert_eq!(config.timeout(), None);
    }

    #[test]
    fn test_timeout_returns_value_when_set() {
        let mut config = ProviderConfig::new();
        config.set_timeout(Duration::from_secs(30));
        assert_eq!(config.timeout(), Some(Duration::from_secs(30)));
    }

    // ── Debug / redaction tests ────────────────────────────

    #[test]
    fn test_debug_redacts_auth_entirely() {
        let mut config = ProviderConfig::new();
        config.set("endpoint", "http://localhost");
        config.set_auth("token", "super-secret-token");
        config.set_auth("password", "hunter2");

        let debug = format!("{config:?}");

        assert!(!debug.contains("super-secret-token"));
        assert!(!debug.contains("hunter2"));
        assert!(debug.contains("[REDACTED]"));
        assert!(debug.contains("http://localhost"));
    }

    #[test]
    fn test_debug_redacts_sensitive_value_keys() {
        let mut config = ProviderConfig::new();
        config.set("endpoint", "http://localhost");
        config.set("access-key-secret", "should-be-redacted");

        let debug = format!("{config:?}");
        assert!(!debug.contains("should-be-redacted"));
        assert!(debug.contains("[REDACTED]"));
    }

    #[test]
    fn test_debug_preserves_non_sensitive_values() {
        let mut config = ProviderConfig::new();
        config.set("endpoint", "http://localhost");
        config.set("project", "my-project");
        config.set("region", "cn-hangzhou");

        let debug = format!("{config:?}");
        assert!(debug.contains("http://localhost"));
        assert!(debug.contains("my-project"));
        assert!(debug.contains("cn-hangzhou"));
    }

    #[test]
    fn test_debug_redacts_sensitive_header_values() {
        let mut config = ProviderConfig::new();
        config.set_header("x-auth-token", "secret-header-val");
        config.set_header("x-custom", "visible-val");

        let debug = format!("{config:?}");
        assert!(!debug.contains("secret-header-val"));
        assert!(debug.contains("visible-val"));
    }

    #[test]
    fn test_sensitive_key_detection_avoids_false_positives() {
        let mut config = ProviderConfig::new();
        config.set("monkey", "banana");
        config.set("hotkey-binding", "ctrl+c");
        config.set("keyboard", "us-layout");

        let debug = format!("{config:?}");
        assert!(debug.contains("banana"));
        assert!(debug.contains("ctrl+c"));
        assert!(debug.contains("us-layout"));
    }

    #[test]
    fn test_default() {
        let config = ProviderConfig::default();
        assert_eq!(config.get("anything"), None);
        assert!(!config.verbose());
        assert_eq!(config.timeout(), None);
    }

    // ── auth_missing_error tests ───────────────────────────

    #[test]
    fn test_auth_missing_error_without_provider_type() {
        let err = auth_missing_error("token", "");
        match err {
            ObzError::InvalidArgument { code, message, .. } => {
                assert_eq!(code, ErrorCode::MissingRequired);
                assert!(message.contains("token"));
                assert!(message.contains("config.yaml"));
            }
            _ => panic!("expected InvalidArgument"),
        }
    }

    #[test]
    fn test_auth_missing_error_with_provider_type() {
        let err = auth_missing_error("api-key", "dd");
        match err {
            ObzError::InvalidArgument { message, .. } => {
                assert!(message.contains("api-key"));
                assert!(message.contains("dd"));
            }
            _ => panic!("expected InvalidArgument"),
        }
    }
}
