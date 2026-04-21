//! SLS API request signing.
//!
//! Wraps the [`aliyun_log_sdk_sign`] crate to sign HTTP requests
//! with the SLS V1 signature algorithm (HMAC-SHA1).
//!
//! The signing function is called just before sending each request.
//! It modifies the request headers in-place, adding:
//! - `Authorization: LOG {AccessKeyID}:{Signature}`
//! - `Date` (RFC 1123 format)
//! - `x-log-apiversion`, `x-log-signaturemethod`
//! - `Content-MD5` (if body is present)

use aliyun_log_sdk_sign::QueryParams;
use http::{HeaderMap, Method};

use obz_core::model::error::{ErrorCode, ObzError};
use obz_core::provider::ProviderResult;

/// Sign an SLS HTTP request in-place.
///
/// # Arguments
///
/// * `access_key_id` — Alibaba Cloud `AccessKey` ID.
/// * `access_key_secret` — Alibaba Cloud `AccessKey` Secret.
/// * `method` — HTTP method (GET, POST, etc.).
/// * `path` — Request path (e.g. `/logstores/my-logs`), without query params.
/// * `headers` — HTTP headers (modified in-place with auth headers).
/// * `query_params` — URL query parameters for inclusion in signature.
/// * `body` — Optional request body for `Content-MD5` calculation.
///
/// # Errors
///
/// Returns [`ObzError::Auth`] if signature computation fails.
pub(crate) fn sign_request(
    access_key_id: &str,
    access_key_secret: &str,
    method: Method,
    path: &str,
    headers: &mut HeaderMap,
    query_params: QueryParams<'_>,
    body: Option<&[u8]>,
) -> ProviderResult<()> {
    aliyun_log_sdk_sign::sign_v1(
        access_key_id,
        access_key_secret,
        None,
        method,
        path,
        headers,
        query_params,
        body,
    )
    .map_err(|e| ObzError::Auth {
        code: ErrorCode::AuthMissing,
        message: format!("failed to sign SLS request: {e}"),
        recoverable: false,
        suggestion: Some(
            "Check access-key-id and access-key-secret in config.yaml auth section".to_string(),
        ),
    })?;
    Ok(())
}
