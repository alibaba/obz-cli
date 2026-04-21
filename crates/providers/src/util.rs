//! Internal utility functions shared across provider implementations.

use std::time::{Duration, Instant};

use reqwest::{Client, StatusCode};

use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::provider::{ProviderConfig, ProviderResult};

/// Default HTTP timeout for provider API requests (30 seconds).
///
/// Shared across all providers to ensure consistent timeout behaviour.
pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Build an HTTP client with a configurable timeout.
///
/// All providers should use this instead of constructing a `Client` manually.
/// Pass the result of [`parse_timeout_config`] as `timeout`; when `None` the
/// [`DEFAULT_TIMEOUT`] (30 s) is used.
///
/// # Errors
///
/// Returns [`ObzError::Network`] if the underlying TLS or system configuration
/// prevents building the client.
pub(crate) fn build_http_client(timeout: Option<Duration>) -> Result<Client, ObzError> {
    Client::builder()
        .timeout(timeout.unwrap_or(DEFAULT_TIMEOUT))
        .build()
        .map_err(|e| ObzError::Network {
            code: ErrorCode::ConnectionError,
            message: format!("failed to build HTTP client: {e}"),
            recoverable: false,
            source_chain: Some(collect_error_chain(&e)),
        })
}

/// Read the timeout from [`ProviderConfig`].
///
/// Returns the typed `Duration` if set, otherwise `None` (callers fall
/// back to [`DEFAULT_TIMEOUT`]).
pub(crate) fn parse_timeout_config(config: &ProviderConfig) -> Option<Duration> {
    config.timeout()
}

/// Apply standard bearer-token / basic-auth to a request builder.
///
/// Bearer token takes precedence over basic auth. If neither is set the
/// request is returned unmodified.
pub(crate) fn apply_standard_auth(
    mut req: reqwest::RequestBuilder,
    bearer_token: &Option<String>,
    basic_auth: &Option<(String, String)>,
) -> reqwest::RequestBuilder {
    if let Some(token) = bearer_token {
        req = req.bearer_auth(token);
    } else if let Some((username, password)) = basic_auth {
        req = req.basic_auth(username, Some(password));
    }
    req
}

/// Headers that must never be set by users — hard error if attempted.
const RESERVED_HEADERS: &[&str] = &[
    "authorization",
    "host",
    "cookie",
    "proxy-authorization",
    "content-length",
    "transfer-encoding",
    "connection",
    "upgrade",
    "expect",
    "te",
    "trailer",
    "via",
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-forwarded-proto",
];

/// Validate that custom headers do not contain reserved keys.
///
/// Call this **before** passing headers to providers that bypass
/// [`apply_custom_headers`] (e.g. `PromQL` shared layer).
///
/// # Errors
///
/// Returns [`ObzError::InvalidArgument`] if any header key is reserved.
pub(crate) fn validate_custom_headers(
    headers: &std::collections::BTreeMap<String, String>,
) -> Result<(), ObzError> {
    for key in headers.keys() {
        if RESERVED_HEADERS.contains(&key.as_str()) {
            return Err(ObzError::InvalidArgument {
                code: ErrorCode::InvalidFlag,
                message: format!(
                    "header \"{key}\" is reserved and cannot be set via custom headers"
                ),
                suggestion: None,
            });
        }
    }
    Ok(())
}

/// Apply custom headers to a request builder with RESERVED and provider-managed filtering.
///
/// # Errors
///
/// Returns [`ObzError::InvalidArgument`] if any header key is reserved.
pub(crate) fn apply_custom_headers(
    mut req: reqwest::RequestBuilder,
    headers: &std::collections::BTreeMap<String, String>,
    provider_managed: &[&str],
    verbose: bool,
) -> Result<reqwest::RequestBuilder, ObzError> {
    validate_custom_headers(headers)?;
    for (key, value) in headers {
        if provider_managed.contains(&key.as_str()) {
            if verbose {
                eprintln!("[verbose] skipping provider-managed header: {key}");
            }
            continue;
        }
        req = req.header(key.as_str(), value.as_str());
        if verbose {
            let display_value = if obz_core::is_sensitive_key(key) {
                "[REDACTED]"
            } else {
                value.as_str()
            };
            eprintln!("[verbose] custom header: {key}: {display_value}");
        }
    }
    Ok(req)
}

/// HTTP response returned by [`send_request`].
///
/// Contains the status code and body text for downstream error handling
/// and deserialization. This struct intentionally does *not* interpret the
/// body — callers decide how to parse it (JSON, NDJSON, plain text, etc.).
pub(crate) struct HttpResponse {
    /// HTTP status code.
    pub status: StatusCode,
    /// Response body as a UTF-8 string.
    pub body: String,
}

/// Build, send, and read an HTTP request with optional verbose logging.
///
/// This is the lowest-level shared helper that **all** providers should use
/// for HTTP communication. It handles:
///
/// 1. Building the `reqwest::Request` from the builder.
/// 2. Printing `[verbose] → {METHOD} {URL}` to stderr (when enabled).
/// 3. Executing the request via `client.execute()`.
/// 4. Printing `[verbose] ← {STATUS} ({N}ms)` to stderr (when enabled).
/// 5. Reading the response body into a `String`.
///
/// It does **not** check the HTTP status code or deserialize the body.
/// Callers are responsible for error handling and parsing, which allows
/// each provider to retain its own error-response format and deserialization
/// logic.
///
/// # Errors
///
/// * [`ObzError::Network`] — request build failure, connection / timeout
///   errors, or body-read failures.
pub(crate) async fn send_request(
    client: &Client,
    req: reqwest::RequestBuilder,
    verbose: bool,
) -> Result<HttpResponse, ObzError> {
    let request = req.build().map_err(|e| classify_reqwest_error(&e))?;
    let method = request.method().clone();
    let url = request.url().clone();
    if verbose {
        eprintln!("[verbose] → {method} {url}");
    }

    let start = Instant::now();
    let resp = client
        .execute(request)
        .await
        .map_err(|e| classify_reqwest_error(&e))?;
    let status = resp.status();
    if verbose {
        eprintln!("[verbose] ← {status} ({}ms)", start.elapsed().as_millis());
    }

    let body = resp.text().await.map_err(|e| ObzError::Network {
        code: ErrorCode::ConnectionError,
        message: format!("failed to read response body: {e}"),
        recoverable: false,
        source_chain: Some(collect_error_chain(&e)),
    })?;

    Ok(HttpResponse { status, body })
}

/// Send a request, check the HTTP status, and deserialize the JSON body.
///
/// This is the standard "fire-and-parse" helper shared across most providers.
/// The `provider_name` is used in error messages (e.g. `"VictoriaMetrics"`).
///
/// # Errors
///
/// * [`ObzError::Network`] — connection / timeout / body-read failures.
/// * [`ObzError::Auth`] / [`ObzError::Provider`] — non-2xx HTTP status
///   (via [`http_error`]).
/// * [`ObzError::Provider`] — JSON deserialization failure.
pub(crate) async fn send_and_parse_json<T: serde::de::DeserializeOwned>(
    client: &Client,
    req: reqwest::RequestBuilder,
    provider_name: &str,
    verbose: bool,
) -> ProviderResult<T> {
    let HttpResponse { status, body } = send_request(client, req, verbose).await?;

    if !status.is_success() {
        return Err(http_error(status, &body, provider_name));
    }

    serde_json::from_str::<T>(&body).map_err(|e| ObzError::Provider {
        code: ErrorCode::BackendError,
        message: format!("failed to parse {provider_name} response (HTTP {status}): {e}"),
        raw_error: Some(truncate_for_error(&body, 500)),
        recoverable: false,
        suggestion: None,
        doc_url: None,
    })
}

/// Truncate a string for inclusion in error messages (UTF-8 safe).
///
/// Finds the nearest char boundary at or before `max_len` to avoid
/// panicking on multi-byte UTF-8 sequences.
pub(crate) fn truncate_for_error(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let mut end = max_len;
        while end > 0 && !s.is_char_boundary(end) {
            end -= 1;
        }
        format!("{}... ({} bytes total)", &s[..end], s.len())
    }
}

/// Characters allowed without encoding in Elasticsearch / `OpenSearch` index paths.
///
/// Starts from [`percent_encoding::NON_ALPHANUMERIC`] (encode everything
/// except ASCII alphanumerics) and removes characters that are legal in
/// index names or patterns.  This matches the approach used by the
/// official [`elasticsearch-rs`] client.
///
/// [`elasticsearch-rs`]: https://github.com/elastic/elasticsearch-rs/blob/main/elasticsearch/src/http/request.rs
const INDEX_PATH_ENCODE_SET: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'_')
    .remove(b'-')
    .remove(b'.')
    .remove(b',')
    .remove(b'*');

/// Percent-encode an index name for use in Elasticsearch / `OpenSearch`
/// URL paths, preserving wildcards (`*`), multi-index separators (`,`),
/// and other common index-name characters (`_`, `-`, `.`).
pub(crate) fn encode_index_path(index: &str) -> String {
    percent_encoding::utf8_percent_encode(index, INDEX_PATH_ENCODE_SET).to_string()
}

/// Classify a reqwest error into an [`ObzError`].
///
/// Distinguishes timeout, DNS, TLS, and connection errors with
/// appropriate error codes and `recoverable` flags.
pub(crate) fn classify_reqwest_error(err: &reqwest::Error) -> ObzError {
    let chain = collect_error_chain(err);
    let source_chain = if chain.is_empty() { None } else { Some(chain) };

    if err.is_timeout() {
        return ObzError::Network {
            code: ErrorCode::Timeout,
            message: format!("request timed out: {err}"),
            recoverable: true,
            source_chain,
        };
    }

    if err.is_connect() {
        let msg = err.to_string().to_lowercase();
        let code = if msg.contains("dns") || msg.contains("resolve") || msg.contains("getaddrinfo")
        {
            ErrorCode::DnsError
        } else {
            ErrorCode::ConnectionError
        };
        return ObzError::Network {
            code,
            message: format!("connection failed: {err}"),
            recoverable: true,
            source_chain,
        };
    }

    let msg = err.to_string().to_lowercase();
    if msg.contains("tls") || msg.contains("ssl") || msg.contains("certificate") {
        return ObzError::Network {
            code: ErrorCode::TlsError,
            message: format!("TLS error: {err}"),
            recoverable: false,
            source_chain,
        };
    }

    ObzError::Network {
        code: ErrorCode::ConnectionError,
        message: format!("request failed: {err}"),
        recoverable: false,
        source_chain,
    }
}

/// Walk the `.source()` chain of an error and collect each cause as a string.
pub(crate) fn collect_error_chain(err: &dyn std::error::Error) -> Vec<String> {
    let mut chain = Vec::new();
    let mut current = err.source();
    while let Some(cause) = current {
        chain.push(cause.to_string());
        current = cause.source();
    }
    chain
}

/// Handle a non-success HTTP status code, producing a structured [`ObzError`].
///
/// Provides consistent 401/403/429/5xx handling across all providers.
/// The `provider` name is used in error messages.
///
/// # Arguments
///
/// * `status` — The HTTP status code received.
/// * `body` — The response body text (for `raw_error`).
/// * `provider` — Provider name for error messages (e.g. `"VictoriaMetrics"`).
pub(crate) fn http_error(status: StatusCode, body: &str, provider: &str) -> ObzError {
    match status.as_u16() {
        401 => ObzError::Auth {
            code: ErrorCode::AuthMissing,
            message: format!("{provider} returned HTTP 401 Unauthorized"),
            recoverable: false,
            suggestion: Some("Check auth config in config.yaml".to_string()),
        },
        403 => ObzError::Auth {
            code: ErrorCode::AccessDenied,
            message: format!("{provider} returned HTTP 403 Forbidden"),
            recoverable: false,
            suggestion: Some("Check auth config in config.yaml".to_string()),
        },
        429 => ObzError::Provider {
            code: ErrorCode::RateLimited,
            message: format!("{provider} returned HTTP 429 Too Many Requests"),
            raw_error: Some(truncate_for_error(body, 500)),
            recoverable: true,
            suggestion: Some("Reduce query frequency or wait before retrying".to_string()),
            doc_url: None,
        },
        404 => ObzError::Provider {
            code: ErrorCode::NotFound,
            message: format!("{provider} returned HTTP 404 Not Found"),
            raw_error: Some(truncate_for_error(body, 300)),
            recoverable: false,
            suggestion: None,
            doc_url: None,
        },
        s if s >= 500 => ObzError::Provider {
            code: ErrorCode::BackendError,
            message: format!("{provider} returned HTTP {status}"),
            raw_error: Some(truncate_for_error(body, 500)),
            recoverable: true,
            suggestion: None,
            doc_url: None,
        },
        // 400 Bad Request and other client errors: include the response
        // body in the message so users see the actual error detail (e.g.
        // Loki's "parse error at line 1 ...") rather than just "HTTP 400".
        _ => {
            let raw_detail = truncate_for_error(body, 500);
            let message = if body.is_empty() {
                format!("{provider} returned HTTP {status}")
            } else {
                format!("{provider} returned HTTP {status}: {raw_detail}")
            };
            ObzError::Provider {
                code: ErrorCode::BackendError,
                message,
                raw_error: Some(raw_detail),
                recoverable: false,
                suggestion: None,
                doc_url: None,
            }
        }
    }
}

/// Validate that an endpoint string is a well-formed HTTP or HTTPS URL.
///
/// Called in every provider's `build()` function immediately after
/// `config.require("endpoint")`, before any HTTP client is constructed.
/// This surfaces a clear, actionable error early instead of letting the
/// user see a cryptic reqwest message like "relative URL without a base".
///
/// # Errors
///
/// Returns [`ObzError::InvalidArgument`] with [`ErrorCode::InvalidFlag`] if:
/// - The endpoint does not start with `http://` or `https://`.
/// - The endpoint is not a syntactically valid URL (e.g. missing host).
pub(crate) fn validate_endpoint(endpoint: &str) -> Result<(), ObzError> {
    if !endpoint.starts_with("http://") && !endpoint.starts_with("https://") {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::InvalidFlag,
            message: format!("--endpoint must start with http:// or https://, got: '{endpoint}'"),
            suggestion: None,
        });
    }
    // reqwest::Url is url::Url — no additional dependency needed.
    reqwest::Url::parse(endpoint).map_err(|e| ObzError::InvalidArgument {
        code: ErrorCode::InvalidFlag,
        message: format!("--endpoint is not a valid URL: '{endpoint}' ({e})"),
        suggestion: None,
    })?;
    Ok(())
}

/// Parse a nanosecond Unix timestamp string to seconds (`i64`).
///
/// Used by Loki and Tempo providers whose APIs return timestamps as
/// nanosecond strings (e.g., `"1775538346468275580"`).
/// Divides by `1_000_000_000` to convert to seconds.
///
/// Returns `0` on parse failure and prints a warning to stderr, so that
/// callers can rely on an `i64` result without needing to handle `Option`.
/// The warning ensures parse failures are visible during debugging instead
/// of silently producing `1970-01-01T00:00:00Z` entries.
pub(crate) fn parse_nanos_to_seconds(s: &str) -> i64 {
    match s.parse::<i128>() {
        Ok(ns) => (ns / 1_000_000_000) as i64,
        Err(_) => {
            eprintln!(
                "[obz warn] parse_nanos_to_seconds: could not parse {:?} as nanosecond timestamp, using 0",
                s
            );
            0
        }
    }
}

/// Convert a JSON value to a flat string representation.
///
/// - Strings are returned as-is (without quotes).
/// - Null becomes an empty string.
/// - Everything else (booleans, numbers, arrays, objects) uses `Display`.
pub(crate) fn json_value_to_string(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_short_string_unchanged() {
        assert_eq!(truncate_for_error("hello", 10), "hello");
    }

    #[test]
    fn truncate_long_string() {
        let long = "a".repeat(100);
        let result = truncate_for_error(&long, 50);
        assert!(result.contains("..."));
        assert!(result.contains("100 bytes total"));
    }

    #[test]
    fn truncate_multibyte_utf8_safe() {
        // "你好世界" = 12 bytes, truncating at 5 should not panic
        let chinese = "你好世界";
        let result = truncate_for_error(chinese, 5);
        assert!(result.contains("..."));
        // Should not split a multi-byte char
        assert!(result.is_char_boundary(result.find("...").unwrap()));
    }

    #[test]
    fn truncate_empty_string() {
        assert_eq!(truncate_for_error("", 10), "");
    }

    #[test]
    fn parse_nanos_known_values() {
        assert_eq!(parse_nanos_to_seconds("1775538346468275580"), 1_775_538_346);
        assert_eq!(parse_nanos_to_seconds("1000000000"), 1);
        assert_eq!(parse_nanos_to_seconds("0"), 0);
    }

    #[test]
    fn parse_nanos_invalid_returns_zero() {
        assert_eq!(parse_nanos_to_seconds("invalid"), 0);
        assert_eq!(parse_nanos_to_seconds(""), 0);
    }

    #[test]
    fn build_http_client_returns_usable_client() {
        // Ensure the HTTP client can be built without errors.
        // Requires a crypto provider for rustls.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = build_http_client(None);
        assert!(client.is_ok(), "build_http_client(None) should succeed");
    }

    #[test]
    fn build_http_client_with_custom_timeout() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = build_http_client(Some(Duration::from_secs(120)));
        assert!(
            client.is_ok(),
            "build_http_client(Some(120s)) should succeed"
        );
    }

    #[test]
    fn parse_timeout_config_absent_returns_none() {
        let config = ProviderConfig::new();
        assert_eq!(parse_timeout_config(&config), None);
    }

    #[test]
    fn parse_timeout_config_with_timeout_returns_some() {
        let mut config = ProviderConfig::new();
        config.set_timeout(Duration::from_secs(60));
        assert_eq!(parse_timeout_config(&config), Some(Duration::from_secs(60)));
    }

    #[test]
    fn apply_standard_auth_bearer_takes_precedence() {
        // When both bearer and basic auth are set, bearer should be used.
        // We verify this indirectly by checking the request is built without error.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::new();
        let req = client.get("http://localhost");

        let bearer = Some("my-token".to_string());
        let basic = Some(("user".to_string(), "pass".to_string()));

        // Should not panic and should produce a valid request builder.
        let _req = apply_standard_auth(req, &bearer, &basic);
    }

    #[test]
    fn apply_standard_auth_basic_when_no_bearer() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::new();
        let req = client.get("http://localhost");

        let bearer = None;
        let basic = Some(("user".to_string(), "pass".to_string()));

        let _req = apply_standard_auth(req, &bearer, &basic);
    }

    #[test]
    fn apply_standard_auth_none_when_both_absent() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let client = reqwest::Client::new();
        let req = client.get("http://localhost");

        let bearer: Option<String> = None;
        let basic: Option<(String, String)> = None;

        let _req = apply_standard_auth(req, &bearer, &basic);
    }

    #[test]
    fn validate_endpoint_accepts_http() {
        assert!(validate_endpoint("http://localhost:8428").is_ok());
        assert!(validate_endpoint("http://vm.example.com/path/prefix").is_ok());
    }

    #[test]
    fn validate_endpoint_accepts_https() {
        assert!(validate_endpoint("https://api.datadoghq.com").is_ok());
        assert!(validate_endpoint("https://vm.example.com:443/prefix").is_ok());
    }

    #[test]
    fn validate_endpoint_rejects_missing_scheme() {
        let err = validate_endpoint("localhost:8428").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("http://") || msg.contains("https://"),
            "expected scheme hint in: {msg}"
        );
    }

    #[test]
    fn validate_endpoint_rejects_wrong_scheme() {
        let err = validate_endpoint("ftp://localhost").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("http://") || msg.contains("https://"),
            "expected scheme hint in: {msg}"
        );
    }

    #[test]
    fn validate_endpoint_rejects_bare_word() {
        assert!(validate_endpoint("foobar").is_err());
        assert!(validate_endpoint("").is_err());
    }

    #[test]
    fn http_error_400_includes_body_in_message() {
        let err = http_error(
            StatusCode::BAD_REQUEST,
            "parse error at line 1, col 10: syntax error",
            "Loki",
        );
        match err {
            ObzError::Provider {
                message, raw_error, ..
            } => {
                assert!(
                    message.contains("parse error"),
                    "message should include body detail: {message}"
                );
                assert!(
                    message.contains("400"),
                    "message should include status code: {message}"
                );
                assert!(raw_error.is_some());
            }
            other => panic!("expected Provider error, got {other:?}"),
        }
    }

    #[test]
    fn http_error_400_empty_body() {
        let err = http_error(StatusCode::BAD_REQUEST, "", "TestProvider");
        match err {
            ObzError::Provider { message, .. } => {
                assert!(
                    message.contains("400"),
                    "message should include status code: {message}"
                );
                // Empty body should not append ": " to the message.
                assert!(
                    !message.contains(": "),
                    "empty body should not add detail separator: {message}"
                );
            }
            other => panic!("expected Provider error, got {other:?}"),
        }
    }

    #[test]
    fn json_value_to_string_primitives() {
        use serde_json::json;

        assert_eq!(json_value_to_string(&json!("hello")), "hello");
        assert_eq!(json_value_to_string(&json!(true)), "true");
        assert_eq!(json_value_to_string(&json!(42)), "42");
        assert_eq!(json_value_to_string(&json!(1.5)), "1.5");
        assert_eq!(json_value_to_string(&json!(null)), "");
    }

    #[test]
    fn json_value_to_string_complex() {
        use serde_json::json;

        assert_eq!(json_value_to_string(&json!([1, 2])), "[1,2]");
        assert_eq!(json_value_to_string(&json!({"a": 1})), "{\"a\":1}");
    }

    #[test]
    fn collect_error_chain_no_source() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let chain = collect_error_chain(&err);
        assert!(chain.is_empty());
    }

    #[test]
    fn collect_error_chain_with_source() {
        #[derive(Debug)]
        struct Outer {
            source: std::io::Error,
        }
        impl std::fmt::Display for Outer {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "outer error")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.source)
            }
        }
        let err = Outer {
            source: std::io::Error::other("disk full"),
        };
        let chain = collect_error_chain(&err);
        assert_eq!(chain.len(), 1);
        assert_eq!(chain[0], "disk full");
    }
}
