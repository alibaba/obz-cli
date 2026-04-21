//! Lightweight health checks shared by provider management commands.

use std::time::{Duration, Instant};

use obz_core::provider::ProviderConfig;
use obz_core::registry::{CheckResult, CheckScope, CheckSeverity};
use reqwest::StatusCode;

use crate::util::{
    apply_custom_headers, apply_standard_auth, build_http_client, classify_reqwest_error,
};

fn inferred_check_scope(config: &ProviderConfig) -> CheckScope {
    if config.bearer_token().is_some() || config.basic_auth().is_some() {
        CheckScope::ConnectivityAndAuth
    } else if config.auth_get("api-key").is_some()
        || config.auth_get("access-key-id").is_some()
        || config.auth_get("access-key-secret").is_some()
    {
        CheckScope::ConfiguredNotVerifiable
    } else {
        CheckScope::Connectivity
    }
}

/// Shared HTTP GET probe for providers whose health endpoint accepts standard auth.
///
/// Sends a GET request to `{endpoint}{path}` with a 5-second timeout, applying
/// any configured bearer-token or basic-auth credentials.
///
/// Returns `severity: Warn` when:
/// - The server returns 5xx (server error — endpoint is reachable but unhealthy)
///
/// Returns `severity: Fail` when:
/// - The endpoint is not configured
/// - The HTTP client cannot be built
/// - The connection fails or times out
/// - The server returns 401/403 (authentication failure)
/// - The server returns an unexpected non-2xx status code
pub async fn http_get_probe(config: &ProviderConfig, path: &str) -> CheckResult {
    let endpoint = match config.get("endpoint") {
        Some(endpoint) => endpoint.trim_end_matches('/'),
        None => {
            return CheckResult {
                severity: CheckSeverity::Fail,
                message: "endpoint not configured".to_string(),
                scope: CheckScope::Connectivity,
                latency: None,
            };
        }
    };

    let url = format!("{endpoint}{path}");
    let client = match build_http_client(Some(Duration::from_secs(5))) {
        Ok(client) => client,
        Err(error) => {
            return CheckResult {
                severity: CheckSeverity::Fail,
                message: error.to_string(),
                scope: CheckScope::Connectivity,
                latency: None,
            };
        }
    };

    let bearer_token = config.bearer_token();
    let basic_auth = config.basic_auth();
    let scope = inferred_check_scope(config);

    let mut request = client.get(&url);
    request = apply_standard_auth(request, &bearer_token, &basic_auth);

    request = match apply_custom_headers(request, config.custom_headers(), &[], false) {
        Ok(request) => request,
        Err(error) => {
            return CheckResult {
                severity: CheckSeverity::Fail,
                message: error.to_string(),
                scope: CheckScope::Connectivity,
                latency: None,
            };
        }
    };

    let start = Instant::now();

    match request.send().await {
        Ok(response) => {
            let latency = Some(start.elapsed());
            let status = response.status();
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                return CheckResult {
                    severity: CheckSeverity::Fail,
                    message: format!("authentication failed (HTTP {status})"),
                    scope,
                    latency,
                };
            }

            if status.is_success() {
                return CheckResult {
                    severity: CheckSeverity::Ok,
                    message: format!("reachable (HTTP {status})"),
                    scope,
                    latency,
                };
            }

            if status.is_server_error() {
                return CheckResult {
                    severity: CheckSeverity::Warn,
                    message: format!("reachable but server error (HTTP {status})"),
                    scope,
                    latency,
                };
            }

            CheckResult {
                severity: CheckSeverity::Fail,
                message: format!("unexpected response (HTTP {status})"),
                scope,
                latency,
            }
        }
        Err(error) => {
            let latency = Some(start.elapsed());
            let classified = classify_reqwest_error(&error);
            let message = if error.is_timeout() {
                "connection timed out after 5s".to_string()
            } else {
                classified.to_string()
            };

            CheckResult {
                severity: CheckSeverity::Fail,
                message,
                scope: CheckScope::Connectivity,
                latency,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{matchers, Mock, MockServer, ResponseTemplate};

    fn ensure_crypto_provider() {
        let _ = rustls::crypto::ring::default_provider().install_default();
    }

    fn unused_local_endpoint() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn http_get_probe_returns_success_for_200() {
        ensure_crypto_provider();
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut config = ProviderConfig::new();
        config.set("endpoint", server.uri());

        let result = http_get_probe(&config, "/health").await;
        assert_eq!(result.severity, CheckSeverity::Ok);
        assert_eq!(result.scope, CheckScope::Connectivity);
        assert_eq!(result.message, "reachable (HTTP 200 OK)");
        assert!(result.latency.is_some());
    }

    #[tokio::test]
    async fn http_get_probe_reports_auth_failure() {
        ensure_crypto_provider();
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::header("authorization", "Bearer secret-token"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let mut config = ProviderConfig::new();
        config.set("endpoint", server.uri());
        config.set_auth("token", "secret-token");

        let result = http_get_probe(&config, "/health").await;
        assert_eq!(result.severity, CheckSeverity::Fail);
        assert_eq!(result.scope, CheckScope::ConnectivityAndAuth);
        assert_eq!(
            result.message,
            "authentication failed (HTTP 401 Unauthorized)"
        );
        assert!(result.latency.is_some());
    }

    #[tokio::test]
    async fn http_get_probe_reports_network_failure() {
        ensure_crypto_provider();
        let mut config = ProviderConfig::new();
        config.set("endpoint", unused_local_endpoint());

        let result = http_get_probe(&config, "/health").await;
        assert_eq!(result.severity, CheckSeverity::Fail);
        assert_eq!(result.scope, CheckScope::Connectivity);
        assert!(result.message.contains("connection failed"));
        assert!(result.latency.is_some());
    }

    #[tokio::test]
    async fn http_get_probe_applies_custom_headers() {
        ensure_crypto_provider();
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .and(matchers::header("x-test-header", "hello"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut config = ProviderConfig::new();
        config.set("endpoint", server.uri());
        config.set_header("x-test-header", "hello");

        let result = http_get_probe(&config, "/health").await;
        assert_eq!(result.severity, CheckSeverity::Ok);
        assert_eq!(result.scope, CheckScope::Connectivity);
        assert_eq!(result.message, "reachable (HTTP 200 OK)");
        assert!(result.latency.is_some());
    }

    #[tokio::test]
    async fn http_get_probe_marks_api_key_as_not_verifiable() {
        ensure_crypto_provider();
        let server = MockServer::start().await;
        Mock::given(matchers::method("GET"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;

        let mut config = ProviderConfig::new();
        config.set("endpoint", server.uri());
        config.set_auth("api-key", "secret");

        let result = http_get_probe(&config, "/health").await;
        assert_eq!(result.severity, CheckSeverity::Ok);
        assert_eq!(result.scope, CheckScope::ConfiguredNotVerifiable);
        assert_eq!(result.message, "reachable (HTTP 200 OK)");
        assert!(result.latency.is_some());
    }
}
