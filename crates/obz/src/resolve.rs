//! Variable reference resolution for config values.
//!
//! Supported references:
//! - `${env:VAR}` — environment variable, error if unset
//! - `${env?:VAR}` — environment variable, empty string if unset
//! - `${file:path}` — file contents, `~` expansion, relative to config dir

use std::path::Path;

use obz_core::{ErrorCode, ObzError};

/// Resolve variable references in a string value.
///
/// Returns the original string unchanged if it contains no `${...}` patterns.
/// Supports multiple references in a single value (e.g. `prefix-${env:A}-${env:B}`).
///
/// # Errors
///
/// - `${env:VAR}` with unset variable
/// - `${file:path}` with missing, empty, or non-UTF-8 file
/// - Nested references like `${env:${file:x}}`
pub(crate) fn resolve_value(value: &str, config_dir: &Path) -> Result<String, ObzError> {
    if !value.contains("${") {
        return Ok(value.to_string());
    }

    let mut result = String::with_capacity(value.len());
    let mut rest = value;

    while let Some(start) = rest.find("${") {
        result.push_str(&rest[..start]);

        let after_dollar = &rest[start + 2..];
        let Some(end) = after_dollar.find('}') else {
            return Err(resolve_error(format!(
                "unclosed variable reference in \"{value}\""
            )));
        };

        let ref_body = &after_dollar[..end];

        if ref_body.contains("${") {
            return Err(resolve_error(format!(
                "nested variable references are not supported: \"{value}\""
            )));
        }

        let resolved = resolve_single_ref(ref_body, config_dir, value)?;
        result.push_str(&resolved);

        rest = &after_dollar[end + 1..];
    }

    result.push_str(rest);
    Ok(result)
}

fn resolve_single_ref(
    ref_body: &str,
    config_dir: &Path,
    original: &str,
) -> Result<String, ObzError> {
    if let Some(var) = ref_body.strip_prefix("env:") {
        resolve_env(var, false)
    } else if let Some(var) = ref_body.strip_prefix("env?:") {
        resolve_env(var, true)
    } else if let Some(path) = ref_body.strip_prefix("file:") {
        resolve_file(path, config_dir)
    } else {
        Err(resolve_error(format!(
            "unknown variable reference \"${{{ref_body}}}\" in \"{original}\". \
             Supported: ${{env:VAR}}, ${{env?:VAR}}, ${{file:path}}"
        )))
    }
}

fn resolve_env(var: &str, optional: bool) -> Result<String, ObzError> {
    if var.is_empty() {
        return Err(resolve_error(
            "empty variable name in ${env:} reference".to_string(),
        ));
    }
    match std::env::var(var) {
        Ok(val) => Ok(val),
        Err(_) if optional => Ok(String::new()),
        Err(_) => Err(resolve_error(format!(
            "environment variable \"{var}\" is not set (use ${{env?:{var}}} to allow unset)"
        ))),
    }
}

fn resolve_file(path: &str, config_dir: &Path) -> Result<String, ObzError> {
    if path.is_empty() {
        return Err(resolve_error(
            "empty path in ${file:} reference".to_string(),
        ));
    }

    let expanded = if let Some(rest) = path.strip_prefix("~/") {
        match home_dir() {
            Some(home) => home.join(rest),
            None => {
                return Err(resolve_error(format!(
                    "cannot expand ~ in \"${{{path}}}\": home directory not found"
                )));
            }
        }
    } else {
        let p = Path::new(path);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            config_dir.join(path)
        }
    };

    let content = std::fs::read_to_string(&expanded).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => resolve_error(format!(
            "file not found: {} (from ${{file:{path}}})",
            expanded.display()
        )),
        std::io::ErrorKind::PermissionDenied => resolve_error(format!(
            "permission denied reading {}: check file permissions (from ${{file:{path}}})",
            expanded.display()
        )),
        _ => resolve_error(format!(
            "failed to read {}: {e} (from ${{file:{path}}})",
            expanded.display()
        )),
    })?;

    if content.is_empty() {
        return Err(resolve_error(format!(
            "file is empty: {} (from ${{file:{path}}})",
            expanded.display()
        )));
    }

    // Trim exactly one trailing newline (LF or CRLF).
    let trimmed = content
        .strip_suffix("\r\n")
        .or_else(|| content.strip_suffix('\n'))
        .unwrap_or(&content);

    Ok(trimmed.to_string())
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

fn resolve_error(message: String) -> ObzError {
    ObzError::InvalidArgument {
        code: ErrorCode::ConfigError,
        message,
        suggestion: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        vars: Vec<(String, Option<String>)>,
    }

    impl EnvGuard {
        fn set(pairs: &[(&str, &str)]) -> Self {
            let mut vars = Vec::new();
            for &(key, val) in pairs {
                let prev = std::env::var(key).ok();
                vars.push((key.to_string(), prev));
                std::env::set_var(key, val);
            }
            Self { vars }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, prev) in &self.vars {
                match prev {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn tmp_config_dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    // ── env: tests ─────────────────────────────────────────

    #[test]
    fn env_var_set_resolves() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::set(&[("OBZ_TEST_TOKEN", "my-secret")]);
        let dir = tmp_config_dir();
        let result = resolve_value("${env:OBZ_TEST_TOKEN}", dir.path()).unwrap();
        assert_eq!(result, "my-secret");
    }

    #[test]
    fn env_var_unset_errors() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("OBZ_UNSET_VAR_12345");
        let dir = tmp_config_dir();
        let err = resolve_value("${env:OBZ_UNSET_VAR_12345}", dir.path()).unwrap_err();
        assert!(err.to_string().contains("not set"));
    }

    #[test]
    fn env_optional_unset_returns_empty() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("OBZ_OPT_VAR_12345");
        let dir = tmp_config_dir();
        let result = resolve_value("${env?:OBZ_OPT_VAR_12345}", dir.path()).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn env_optional_set_returns_value() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::set(&[("OBZ_OPT_SET", "val")]);
        let dir = tmp_config_dir();
        let result = resolve_value("${env?:OBZ_OPT_SET}", dir.path()).unwrap();
        assert_eq!(result, "val");
    }

    #[test]
    fn env_empty_var_name_errors() {
        let dir = tmp_config_dir();
        let err = resolve_value("${env:}", dir.path()).unwrap_err();
        assert!(err.to_string().contains("empty variable name"));
    }

    // ── file: tests ────────────────────────────────────────

    #[test]
    fn file_reads_content() {
        let dir = tmp_config_dir();
        std::fs::write(dir.path().join("secret.txt"), "file-secret\n").unwrap();
        let result = resolve_value("${file:secret.txt}", dir.path()).unwrap();
        assert_eq!(result, "file-secret");
    }

    #[test]
    fn file_trims_crlf() {
        let dir = tmp_config_dir();
        std::fs::write(dir.path().join("crlf.txt"), "value\r\n").unwrap();
        let result = resolve_value("${file:crlf.txt}", dir.path()).unwrap();
        assert_eq!(result, "value");
    }

    #[test]
    fn file_no_trailing_newline_unchanged() {
        let dir = tmp_config_dir();
        std::fs::write(dir.path().join("no-nl.txt"), "value").unwrap();
        let result = resolve_value("${file:no-nl.txt}", dir.path()).unwrap();
        assert_eq!(result, "value");
    }

    #[test]
    fn file_tilde_expansion() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let dir = tmp_config_dir();
        let home = dir.path().to_str().unwrap();
        let prev_home = std::env::var("HOME").ok();
        std::env::set_var("HOME", home);
        std::fs::write(dir.path().join("token.txt"), "tilde-token\n").unwrap();

        let result = resolve_value("${file:~/token.txt}", dir.path()).unwrap();
        assert_eq!(result, "tilde-token");

        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }

    #[test]
    fn file_relative_to_config_dir() {
        let dir = tmp_config_dir();
        std::fs::write(dir.path().join("rel.key"), "rel-value\n").unwrap();
        let result = resolve_value("${file:rel.key}", dir.path()).unwrap();
        assert_eq!(result, "rel-value");
    }

    #[test]
    fn file_not_found_errors() {
        let dir = tmp_config_dir();
        let err = resolve_value("${file:nonexistent.key}", dir.path()).unwrap_err();
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn file_empty_errors() {
        let dir = tmp_config_dir();
        std::fs::write(dir.path().join("empty.txt"), "").unwrap();
        let err = resolve_value("${file:empty.txt}", dir.path()).unwrap_err();
        assert!(err.to_string().contains("empty"));
    }

    #[test]
    fn file_empty_path_errors() {
        let dir = tmp_config_dir();
        let err = resolve_value("${file:}", dir.path()).unwrap_err();
        assert!(err.to_string().contains("empty path"));
    }

    // ── Mixed / edge cases ─────────────────────────────────

    #[test]
    fn no_reference_passthrough() {
        let dir = tmp_config_dir();
        let result = resolve_value("plain-string", dir.path()).unwrap();
        assert_eq!(result, "plain-string");
    }

    #[test]
    fn mixed_text_and_env() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::set(&[("OBZ_MIX_VAR", "injected")]);
        let dir = tmp_config_dir();
        let result = resolve_value("prefix-${env:OBZ_MIX_VAR}-suffix", dir.path()).unwrap();
        assert_eq!(result, "prefix-injected-suffix");
    }

    #[test]
    fn multiple_references() {
        let _lock = ENV_MUTEX.lock().unwrap();
        let _guard = EnvGuard::set(&[("OBZ_A", "one"), ("OBZ_B", "two")]);
        let dir = tmp_config_dir();
        let result = resolve_value("${env:OBZ_A}-${env:OBZ_B}", dir.path()).unwrap();
        assert_eq!(result, "one-two");
    }

    #[test]
    fn nested_reference_errors() {
        let dir = tmp_config_dir();
        let err = resolve_value("${env:${file:x}}", dir.path()).unwrap_err();
        assert!(err.to_string().contains("nested"));
    }

    #[test]
    fn unclosed_reference_errors() {
        let dir = tmp_config_dir();
        let err = resolve_value("${env:FOO", dir.path()).unwrap_err();
        assert!(err.to_string().contains("unclosed"));
    }

    #[test]
    fn unknown_reference_type_errors() {
        let dir = tmp_config_dir();
        let err = resolve_value("${cmd:echo hi}", dir.path()).unwrap_err();
        assert!(err.to_string().contains("unknown"));
    }

    // ── Path policy tests (by design: allow any readable path) ──

    #[test]
    fn file_relative_parent_traversal_allowed() {
        let dir = tmp_config_dir();
        let parent = dir.path().parent().unwrap();
        std::fs::write(parent.join("traversal-test.key"), "traversed\n").unwrap();

        let result = resolve_value("${file:../traversal-test.key}", dir.path()).unwrap();
        assert_eq!(result, "traversed");

        let _ = std::fs::remove_file(parent.join("traversal-test.key"));
    }

    #[test]
    fn file_absolute_path_allowed() {
        let dir = tmp_config_dir();
        let abs_file = dir.path().join("abs-test.key");
        std::fs::write(&abs_file, "absolute-value\n").unwrap();

        let abs_path = abs_file.to_str().unwrap();
        let ref_str = format!("${{file:{abs_path}}}");
        let result = resolve_value(&ref_str, dir.path()).unwrap();
        assert_eq!(result, "absolute-value");
    }
}
