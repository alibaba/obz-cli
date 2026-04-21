//! External credential process support.
//!
//! Executes an external command to dynamically obtain authentication
//! credentials, following a JSON protocol inspired by AWS CLI's
//! `credential_process`. The command is run directly (no shell) to avoid
//! quoting/injection issues.
//!
//! # JSON Protocol
//!
//! The external command must print a JSON object to stdout:
//!
//! ```json
//! {
//!   "version": 1,
//!   "expiration": "2026-04-17T12:00:00Z",
//!   "token": "bearer-token",
//!   "username": "optional",
//!   "password": "optional",
//!   "access-key-id": "optional",
//!   "access-key-secret": "optional",
//!   "api-key": "optional",
//!   "app-key": "optional",
//!   "headers": {
//!     "x-custom-header": "value"
//!   }
//! }
//! ```
//!
//! - `version` must be `1`.
//! - `expiration` is used by disk cache expiration logic.
//! - Auth fields with non-empty values override config inline values.
//! - `headers` are merged with config headers (credential-process wins
//!   on conflict).
//! - Empty-string fields are treated as absent and do not override.

use std::collections::BTreeMap;
use std::process::Command;
use std::time::Duration;

use obz_core::{ErrorCode, ObzError};
use serde::Deserialize;

pub(crate) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum bytes of stderr to include in error messages.
const MAX_STDERR_BYTES: usize = 512;

/// Known auth field names in the credential process output.
const AUTH_FIELDS: &[&str] = &[
    "token",
    "username",
    "password",
    "access-key-id",
    "access-key-secret",
    "api-key",
    "app-key",
];

/// Configuration for an external credential process.
#[derive(Debug, Clone)]
pub(crate) struct CredentialProcessConfig {
    /// Command to execute (no shell — `Command::new` directly).
    pub(crate) command: String,
    /// Arguments passed to the command.
    pub(crate) args: Vec<String>,
    /// Maximum time to wait for the process to complete.
    pub(crate) timeout: Duration,
    /// Optional fallback cache TTL when protocol output has no expiration.
    pub(crate) cache_ttl: Option<Duration>,
}

/// Output from a successful credential process execution.
#[derive(Debug)]
pub(crate) struct CredentialOutput {
    /// Auth key-value pairs (e.g. `token`, `username`, `api-key`).
    pub(crate) auth: BTreeMap<String, String>,
    /// Custom HTTP headers (keys lowercased).
    pub(crate) headers: BTreeMap<String, String>,
    /// Optional expiration timestamp from the protocol output.
    pub(crate) expiration: Option<String>,
}

/// Raw JSON output from the credential process.
#[derive(Deserialize)]
struct RawOutput {
    version: u32,
    expiration: Option<String>,
    #[serde(flatten)]
    fields: serde_json::Map<String, serde_json::Value>,
}

/// Execute the credential process and parse its JSON output.
///
/// Runs the configured command directly (no shell), reads stdout as JSON,
/// validates the protocol version, and extracts auth fields and headers.
///
/// # Errors
///
/// - Command not found → `InvalidArgument`
/// - Non-zero exit code → `Auth` (with truncated stderr)
/// - Timeout → `Network` with `Timeout` code, child process killed
/// - Invalid JSON → `InvalidArgument`
/// - Wrong version → `InvalidArgument`
pub(crate) fn execute(config: &CredentialProcessConfig) -> Result<CredentialOutput, ObzError> {
    let mut child = Command::new(&config.command)
        .args(&config.args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => ObzError::InvalidArgument {
                code: ErrorCode::ConfigError,
                message: format!(
                    "credential-process command not found: \"{}\"",
                    config.command
                ),
                suggestion: Some(
                    "Check credential-process.command in config.yaml. The command must be in $PATH"
                        .to_string(),
                ),
            },
            std::io::ErrorKind::PermissionDenied => ObzError::InvalidArgument {
                code: ErrorCode::ConfigError,
                message: format!(
                    "credential-process command not executable: \"{}\"",
                    config.command
                ),
                suggestion: None,
            },
            _ => ObzError::InvalidArgument {
                code: ErrorCode::ConfigError,
                message: format!(
                    "failed to start credential-process \"{}\": {e}",
                    config.command
                ),
                suggestion: None,
            },
        })?;

    // Take pipes BEFORE waiting to avoid pipe-buffer deadlock.
    // If the child writes more than the OS pipe buffer (typically 64 KB),
    // it blocks until the parent reads; if the parent is waiting for exit
    // first, both sides deadlock.
    let stdout_pipe = child.stdout.take();
    let stderr_pipe = child.stderr.take();

    // Read stdout and stderr on background threads so they are consumed
    // concurrently, preventing pipe-buffer deadlock regardless of output size.
    let stdout_handle = std::thread::spawn(move || -> Result<String, ObzError> {
        use std::io::Read;
        let mut buf = String::new();
        if let Some(mut pipe) = stdout_pipe {
            pipe.read_to_string(&mut buf)
                .map_err(|e| ObzError::InvalidArgument {
                    code: ErrorCode::ConfigError,
                    message: format!("failed to read credential-process stdout: {e}"),
                    suggestion: None,
                })?;
        }
        Ok(buf)
    });

    let stderr_handle =
        stderr_pipe.map(|pipe| std::thread::spawn(move || read_pipe_bytes(pipe, MAX_STDERR_BYTES)));

    let timeout = config.timeout;
    let status = wait_with_timeout(&mut child, timeout).map_err(|_| {
        let _ = child.kill();
        let _ = child.wait();
        ObzError::Auth {
            code: ErrorCode::AuthMissing,
            message: format!(
                "credential-process \"{}\" timed out after {:.0}s",
                config.command,
                timeout.as_secs_f64()
            ),
            recoverable: false,
            suggestion: Some(format!(
                "Check if the command is hanging, or increase timeout \
                 (currently {:.0}s) in config.yaml",
                timeout.as_secs_f64()
            )),
        }
    })?;

    let stdout = stdout_handle
        .join()
        .map_err(|_| ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: "credential-process stdout reader thread panicked".to_string(),
            suggestion: None,
        })??;

    if !status.success() {
        let stderr = stderr_handle
            .and_then(|h| h.join().ok())
            .unwrap_or_default();
        let exit_info = match status.code() {
            Some(code) => format!("exit code {code}"),
            None => "killed by signal".to_string(),
        };
        return Err(ObzError::Auth {
            code: ErrorCode::AuthMissing,
            message: format!(
                "credential-process \"{}\" failed ({exit_info}): {stderr}",
                config.command
            ),
            recoverable: false,
            suggestion: Some("Check the credential-process command and its arguments".to_string()),
        });
    }

    parse_output(&stdout, &config.command)
}

/// Parse the JSON output from a credential process.
fn parse_output(stdout: &str, command: &str) -> Result<CredentialOutput, ObzError> {
    let raw: RawOutput = serde_json::from_str(stdout).map_err(|e| ObzError::InvalidArgument {
        code: ErrorCode::ConfigError,
        message: format!("credential-process \"{command}\" returned invalid JSON: {e}"),
        suggestion: None,
    })?;

    if raw.version != 1 {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: format!(
                "credential-process \"{command}\" returned unsupported version {} (expected 1)",
                raw.version
            ),
            suggestion: None,
        });
    }

    let mut auth = BTreeMap::new();
    let mut headers = BTreeMap::new();

    for (key, value) in &raw.fields {
        if key == "headers" {
            match value {
                serde_json::Value::Object(map) => {
                    for (hk, hv) in map {
                        match hv {
                            serde_json::Value::String(s) if !s.is_empty() => {
                                headers.insert(hk.to_ascii_lowercase(), s.clone());
                            }
                            serde_json::Value::String(_) | serde_json::Value::Null => {}
                            _ => {
                                return Err(ObzError::InvalidArgument {
                                    code: ErrorCode::ConfigError,
                                    message: format!(
                                        "credential-process \"{command}\": \
                                         header \"{hk}\" must be a string, got {hv}"
                                    ),
                                    suggestion: None,
                                });
                            }
                        }
                    }
                }
                serde_json::Value::Null => {}
                _ => {
                    return Err(ObzError::InvalidArgument {
                        code: ErrorCode::ConfigError,
                        message: format!(
                            "credential-process \"{command}\": \
                             \"headers\" must be an object"
                        ),
                        suggestion: None,
                    });
                }
            }
            continue;
        }

        if AUTH_FIELDS.contains(&key.as_str()) {
            match value {
                serde_json::Value::String(s) if !s.is_empty() => {
                    auth.insert(key.clone(), s.clone());
                }
                serde_json::Value::String(_) | serde_json::Value::Null => {}
                _ => {
                    return Err(ObzError::InvalidArgument {
                        code: ErrorCode::ConfigError,
                        message: format!(
                            "credential-process \"{command}\": \
                             \"{key}\" must be a string, got {value}"
                        ),
                        suggestion: None,
                    });
                }
            }
            continue;
        }
        // Unknown fields: silently ignored for forward compatibility.
    }

    Ok(CredentialOutput {
        auth,
        headers,
        expiration: raw.expiration,
    })
}

/// Wait for a child process with a timeout.
///
/// Uses a polling loop with exponential backoff to avoid busy-waiting
/// while keeping latency low for fast commands.
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, ()> {
    let start = std::time::Instant::now();
    let mut sleep_ms = 5u64;

    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(status),
            Ok(None) => {
                if start.elapsed() >= timeout {
                    return Err(());
                }
                let remaining = timeout.saturating_sub(start.elapsed());
                let sleep = Duration::from_millis(sleep_ms).min(remaining);
                std::thread::sleep(sleep);
                sleep_ms = (sleep_ms * 2).min(200);
            }
            Err(_) => return Err(()),
        }
    }
}

/// Read from a pipe up to `max_bytes`, returning a trimmed lossy UTF-8 string.
fn read_pipe_bytes(mut pipe: std::process::ChildStderr, max_bytes: usize) -> String {
    use std::io::Read;

    let mut buf = vec![0u8; max_bytes + 1];
    let n = pipe.read(&mut buf).unwrap_or(0);
    let truncated = n > max_bytes;
    let raw = &buf[..n.min(max_bytes)];

    let mut s = String::from_utf8_lossy(raw).to_string();
    if truncated {
        s.push_str("... (truncated)");
    }

    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_output tests ─────────────────────────────────

    #[test]
    fn parse_valid_token_output() {
        let json = r#"{"version": 1, "token": "my-secret"}"#;
        let out = parse_output(json, "test").unwrap();
        assert_eq!(out.auth.get("token"), Some(&"my-secret".to_string()));
        assert!(out.headers.is_empty());
        assert_eq!(out.expiration, None);
    }

    #[test]
    fn parse_full_output_with_all_fields() {
        let json = r#"{
            "version": 1,
            "expiration": "2026-04-17T12:00:00Z",
            "token": "bearer-tok",
            "username": "admin",
            "password": "pass123",
            "access-key-id": "AKID",
            "access-key-secret": "AKSECRET",
            "api-key": "apikey",
            "app-key": "appkey",
            "headers": {
                "X-Custom": "val1",
                "X-Another": "val2"
            }
        }"#;
        let out = parse_output(json, "test").unwrap();
        assert_eq!(out.auth.len(), 7);
        assert_eq!(out.auth.get("token"), Some(&"bearer-tok".to_string()));
        assert_eq!(out.auth.get("username"), Some(&"admin".to_string()));
        assert_eq!(out.auth.get("password"), Some(&"pass123".to_string()));
        assert_eq!(out.auth.get("access-key-id"), Some(&"AKID".to_string()));
        assert_eq!(
            out.auth.get("access-key-secret"),
            Some(&"AKSECRET".to_string())
        );
        assert_eq!(out.auth.get("api-key"), Some(&"apikey".to_string()));
        assert_eq!(out.auth.get("app-key"), Some(&"appkey".to_string()));
        assert_eq!(out.headers.len(), 2);
        assert_eq!(out.headers.get("x-custom"), Some(&"val1".to_string()));
        assert_eq!(out.headers.get("x-another"), Some(&"val2".to_string()));
        assert_eq!(out.expiration.as_deref(), Some("2026-04-17T12:00:00Z"));
    }

    #[test]
    fn parse_empty_string_fields_ignored() {
        let json = r#"{"version": 1, "token": "", "username": "admin"}"#;
        let out = parse_output(json, "test").unwrap();
        assert_eq!(out.auth.get("token"), None);
        assert_eq!(out.auth.get("username"), Some(&"admin".to_string()));
    }

    #[test]
    fn parse_null_fields_ignored() {
        let json = r#"{"version": 1, "token": null, "username": "admin"}"#;
        let out = parse_output(json, "test").unwrap();
        assert_eq!(out.auth.get("token"), None);
        assert_eq!(out.auth.get("username"), Some(&"admin".to_string()));
    }

    #[test]
    fn parse_unknown_fields_ignored() {
        let json = r#"{"version": 1, "token": "tok", "custom-field": "ignored"}"#;
        let out = parse_output(json, "test").unwrap();
        assert_eq!(out.auth.len(), 1);
        assert_eq!(out.auth.get("token"), Some(&"tok".to_string()));
    }

    #[test]
    fn parse_headers_lowercased() {
        let json = r#"{"version": 1, "headers": {"X-Scope-OrgID": "my-tenant"}}"#;
        let out = parse_output(json, "test").unwrap();
        assert_eq!(
            out.headers.get("x-scope-orgid"),
            Some(&"my-tenant".to_string())
        );
    }

    #[test]
    fn parse_empty_headers_object() {
        let json = r#"{"version": 1, "headers": {}}"#;
        let out = parse_output(json, "test").unwrap();
        assert!(out.headers.is_empty());
    }

    #[test]
    fn parse_version_mismatch_errors() {
        let json = r#"{"version": 2, "token": "tok"}"#;
        let err = parse_output(json, "test").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("version"), "error: {msg}");
        assert!(msg.contains('2'), "error: {msg}");
    }

    #[test]
    fn parse_invalid_json_errors() {
        let err = parse_output("not json", "test").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid JSON"), "error: {msg}");
    }

    #[test]
    fn parse_missing_version_errors() {
        let json = r#"{"token": "tok"}"#;
        let err = parse_output(json, "test").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("invalid JSON") || msg.contains("version"),
            "error: {msg}"
        );
    }

    #[test]
    fn parse_only_headers_no_auth() {
        let json = r#"{"version": 1, "headers": {"x-custom": "val"}}"#;
        let out = parse_output(json, "test").unwrap();
        assert!(out.auth.is_empty());
        assert_eq!(out.headers.get("x-custom"), Some(&"val".to_string()));
    }

    #[test]
    fn parse_empty_header_values_ignored() {
        let json = r#"{"version": 1, "headers": {"x-custom": "", "x-other": "val"}}"#;
        let out = parse_output(json, "test").unwrap();
        assert_eq!(out.headers.get("x-custom"), None);
        assert_eq!(out.headers.get("x-other"), Some(&"val".to_string()));
    }

    // ── execute tests (real process) ───────────────────────

    #[test]
    fn execute_echo_command() {
        let config = CredentialProcessConfig {
            command: "echo".to_string(),
            args: vec![r#"{"version": 1, "token": "hello"}"#.to_string()],
            timeout: DEFAULT_TIMEOUT,
            cache_ttl: None,
        };
        let out = execute(&config).unwrap();
        assert_eq!(out.auth.get("token"), Some(&"hello".to_string()));
    }

    #[test]
    fn execute_command_not_found() {
        let config = CredentialProcessConfig {
            command: "/nonexistent/binary/path".to_string(),
            args: vec![],
            timeout: DEFAULT_TIMEOUT,
            cache_ttl: None,
        };
        let err = execute(&config).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("not found"), "error: {msg}");
    }

    #[test]
    fn execute_nonzero_exit_code() {
        let config = CredentialProcessConfig {
            command: "sh".to_string(),
            args: vec![
                "-c".to_string(),
                "echo 'auth failed' >&2; exit 1".to_string(),
            ],
            timeout: DEFAULT_TIMEOUT,
            cache_ttl: None,
        };
        let err = execute(&config).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("exit code 1"), "error: {msg}");
        assert!(msg.contains("auth failed"), "error: {msg}");
    }

    #[test]
    fn execute_invalid_json_output() {
        let config = CredentialProcessConfig {
            command: "echo".to_string(),
            args: vec!["not-json".to_string()],
            timeout: DEFAULT_TIMEOUT,
            cache_ttl: None,
        };
        let err = execute(&config).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid JSON"), "error: {msg}");
    }

    #[test]
    fn execute_timeout() {
        let config = CredentialProcessConfig {
            command: "sleep".to_string(),
            args: vec!["10".to_string()],
            timeout: Duration::from_millis(100),
            cache_ttl: None,
        };
        let err = execute(&config).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("timed out"), "error: {msg}");
        assert!(
            msg.contains("authentication error"),
            "timeout should be auth error, not network: {msg}"
        );
    }

    // ── Type validation tests ───────────────────────────────

    #[test]
    fn parse_auth_field_wrong_type_errors() {
        let json = r#"{"version": 1, "token": 123}"#;
        let err = parse_output(json, "test").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("token"), "error: {msg}");
        assert!(msg.contains("string"), "error: {msg}");
    }

    #[test]
    fn parse_headers_wrong_type_errors() {
        let json = r#"{"version": 1, "headers": "not-object"}"#;
        let err = parse_output(json, "test").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("headers"), "error: {msg}");
        assert!(msg.contains("object"), "error: {msg}");
    }

    #[test]
    fn parse_header_value_wrong_type_errors() {
        let json = r#"{"version": 1, "headers": {"x-custom": 42}}"#;
        let err = parse_output(json, "test").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("x-custom"), "error: {msg}");
        assert!(msg.contains("string"), "error: {msg}");
    }

    #[test]
    fn parse_auth_field_boolean_type_errors() {
        let json = r#"{"version": 1, "username": true}"#;
        let err = parse_output(json, "test").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("username"), "error: {msg}");
    }

    #[test]
    fn parse_null_auth_field_accepted() {
        let json = r#"{"version": 1, "token": null, "username": "admin"}"#;
        let out = parse_output(json, "test").unwrap();
        assert_eq!(out.auth.get("token"), None);
        assert_eq!(out.auth.get("username"), Some(&"admin".to_string()));
    }

    #[test]
    fn parse_null_headers_accepted() {
        let json = r#"{"version": 1, "headers": null}"#;
        let out = parse_output(json, "test").unwrap();
        assert!(out.headers.is_empty());
    }

    // ── Edge case execute tests ───────────────────────────

    #[test]
    fn execute_large_stdout_no_deadlock() {
        // Generate >64KB of valid JSON to verify no pipe deadlock.
        let big_value = "x".repeat(100_000);
        let json = format!(r#"{{"version": 1, "token": "{big_value}"}}"#);
        let config = CredentialProcessConfig {
            command: "echo".to_string(),
            args: vec![json],
            timeout: Duration::from_secs(5),
            cache_ttl: None,
        };
        let out = execute(&config).unwrap();
        assert_eq!(out.auth.get("token").map(String::len), Some(100_000));
    }

    #[test]
    fn execute_empty_stdout_errors() {
        let config = CredentialProcessConfig {
            command: "true".to_string(),
            args: vec![],
            timeout: DEFAULT_TIMEOUT,
            cache_ttl: None,
        };
        let err = execute(&config).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("invalid JSON"), "error: {msg}");
    }

    // ── CredentialProcessConfig defaults ───────────────────

    #[test]
    fn default_timeout_is_30s() {
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(30));
    }
}
