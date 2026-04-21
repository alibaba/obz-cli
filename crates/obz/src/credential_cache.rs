//! Disk-based caching for credential-process output.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use fd_lock::RwLock;
use obz_core::{ErrorCode, ObzError};
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::credential_process::{self, CredentialOutput, CredentialProcessConfig};

const CACHE_VERSION: u32 = 1;
const EXPIRATION_BUFFER_SECS: i64 = 60;

#[derive(Debug, Serialize, Deserialize)]
struct CacheEntry {
    version: u32,
    cached_at: i64,
    expires_at: Option<i64>,
    auth: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
}

pub(crate) fn get_or_refresh(
    provider_name: &str,
    cp_config: &CredentialProcessConfig,
    verbose: bool,
) -> Result<CredentialOutput, ObzError> {
    get_or_refresh_with_cache_dir(provider_name, cp_config, verbose, &cache_dir())
}

pub(crate) fn get_or_refresh_with_cache_dir(
    provider_name: &str,
    cp_config: &CredentialProcessConfig,
    verbose: bool,
    cache_dir: &Path,
) -> Result<CredentialOutput, ObzError> {
    get_or_refresh_impl(provider_name, cp_config, verbose, cache_dir)
}

pub(crate) fn refresh(
    provider_name: &str,
    cp_config: &CredentialProcessConfig,
    verbose: bool,
) -> Result<(), ObzError> {
    refresh_with_cache_dir(provider_name, cp_config, verbose, &cache_dir())
}

pub(crate) fn refresh_with_cache_dir(
    provider_name: &str,
    cp_config: &CredentialProcessConfig,
    verbose: bool,
    cache_dir: &Path,
) -> Result<(), ObzError> {
    refresh_impl(provider_name, cp_config, verbose, cache_dir)
}

/// Removes the cached credential file for the given provider.
///
/// Caller must hold the provider's cache write lock.
fn invalidate_in(provider_name: &str, cache_dir: &Path) {
    let path = cache_dir.join(format!("{provider_name}.json"));
    let _ = fs::remove_file(path);
}

/// Validate that provider name is safe for use as a filename.
///
/// Only `[A-Za-z0-9._-]` are allowed.
fn validate_provider_name(name: &str) -> Result<(), ObzError> {
    if name.is_empty() {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: "provider name cannot be empty".to_string(),
            suggestion: None,
        });
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'_' || b == b'-')
    {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: format!(
                "provider name \"{name}\" contains invalid characters (only alphanumeric, '.', '_', '-' allowed)"
            ),
            suggestion: None,
        });
    }
    Ok(())
}

fn get_or_refresh_impl(
    provider_name: &str,
    cp_config: &CredentialProcessConfig,
    verbose: bool,
    cache_dir: &Path,
) -> Result<CredentialOutput, ObzError> {
    validate_provider_name(provider_name)?;
    let cache_path = cache_path(cache_dir, provider_name);
    let now = now_unix_seconds()?;

    if let Some(entry) = read_cache_if_valid(&cache_path, now) {
        if verbose {
            eprintln!("[verbose] using cached credentials for \"{provider_name}\"");
        }
        return Ok(output_from_entry(entry));
    }

    fs::create_dir_all(cache_dir)
        .map_err(|e| cache_io_error(cache_dir, "create cache directory", &e))?;
    set_dir_permissions(cache_dir)?;
    let lock_path = lock_path(cache_dir, provider_name);
    let mut lock = open_lock(&lock_path)?;
    let _guard = lock
        .write()
        .map_err(|e| cache_io_error(&lock_path, "lock cache file", &e))?;

    let now = now_unix_seconds()?;
    if let Some(entry) = read_cache_if_valid(&cache_path, now) {
        if verbose {
            eprintln!("[verbose] using cached credentials for \"{provider_name}\"");
        }
        return Ok(output_from_entry(entry));
    }

    let output = credential_process::execute(cp_config)?;
    let entry = build_cache_entry(&output, cp_config, now)?;
    write_cache_entry(&cache_path, &entry)?;
    Ok(output)
}

fn refresh_impl(
    provider_name: &str,
    cp_config: &CredentialProcessConfig,
    verbose: bool,
    cache_dir: &Path,
) -> Result<(), ObzError> {
    validate_provider_name(provider_name)?;
    fs::create_dir_all(cache_dir)
        .map_err(|e| cache_io_error(cache_dir, "create cache directory", &e))?;
    set_dir_permissions(cache_dir)?;

    let cache_path = cache_path(cache_dir, provider_name);
    let lock_path = lock_path(cache_dir, provider_name);
    let mut lock = open_lock(&lock_path)?;
    let _guard = lock
        .write()
        .map_err(|e| cache_io_error(&lock_path, "lock cache file", &e))?;

    invalidate_in(provider_name, cache_dir);
    let now = now_unix_seconds()?;
    let output = credential_process::execute(cp_config)?;
    let entry = build_cache_entry(&output, cp_config, now)?;
    if verbose {
        eprintln!("[verbose] refreshed cached credentials for \"{provider_name}\"");
    }
    write_cache_entry(&cache_path, &entry)
}

fn build_cache_entry(
    output: &CredentialOutput,
    cp_config: &CredentialProcessConfig,
    now: i64,
) -> Result<CacheEntry, ObzError> {
    let expires_at = match output.expiration.as_deref() {
        Some(expiration) => Some(parse_expiration(expiration)?),
        None => cp_config
            .cache_ttl
            .map(duration_to_i64_secs)
            .transpose()?
            .map(|ttl| now.saturating_add(ttl)),
    };

    Ok(CacheEntry {
        version: CACHE_VERSION,
        cached_at: now,
        expires_at,
        auth: output.auth.clone(),
        headers: output.headers.clone(),
    })
}

fn read_cache_if_valid(path: &Path, now: i64) -> Option<CacheEntry> {
    let entry = read_cache_entry(path)?;
    if entry.version != CACHE_VERSION {
        return None;
    }

    match entry.expires_at {
        Some(expires_at) if now >= expires_at.saturating_sub(EXPIRATION_BUFFER_SECS) => None,
        _ => Some(entry),
    }
}

fn read_cache_entry(path: &Path) -> Option<CacheEntry> {
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

fn write_cache_entry(path: &Path, entry: &CacheEntry) -> Result<(), ObzError> {
    let parent = path.parent().ok_or_else(|| ObzError::InvalidArgument {
        code: ErrorCode::ConfigError,
        message: format!("cache path has no parent: {}", path.display()),
        suggestion: None,
    })?;
    fs::create_dir_all(parent).map_err(|e| cache_io_error(parent, "create cache directory", &e))?;
    set_dir_permissions(parent)?;

    let mut temp = NamedTempFile::new_in(parent)
        .map_err(|e| cache_io_error(parent, "create temporary cache file", &e))?;
    set_private_permissions(temp.as_file(), path)?;
    serde_json::to_writer(&mut temp, entry).map_err(|e| ObzError::InvalidArgument {
        code: ErrorCode::ConfigError,
        message: format!(
            "failed to serialize credential cache {}: {e}",
            path.display()
        ),
        suggestion: None,
    })?;
    temp.write_all(b"\n")
        .map_err(|e| cache_io_error(path, "write credential cache", &e))?;
    temp.as_file_mut()
        .sync_all()
        .map_err(|e| cache_io_error(path, "sync credential cache", &e))?;

    temp.persist(path)
        .map_err(|e| cache_io_error(path, "persist credential cache", &e.error))?;
    Ok(())
}

fn output_from_entry(entry: CacheEntry) -> CredentialOutput {
    CredentialOutput {
        auth: entry.auth,
        headers: entry.headers,
        expiration: None,
    }
}

fn parse_expiration(s: &str) -> Result<i64, ObzError> {
    jiff::Timestamp::strptime("%Y-%m-%dT%H:%M:%SZ", s)
        .or_else(|_| jiff::Timestamp::strptime("%Y-%m-%dT%H:%M:%S%.fZ", s))
        .or_else(|_| s.parse::<jiff::Timestamp>())
        .map(jiff::Timestamp::as_second)
        .map_err(|e| ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: format!("invalid expiration timestamp \"{s}\": {e}"),
            suggestion: None,
        })
}

fn duration_to_i64_secs(duration: Duration) -> Result<i64, ObzError> {
    i64::try_from(duration.as_secs()).map_err(|_| ObzError::InvalidArgument {
        code: ErrorCode::ConfigError,
        message: "credential-process cache TTL is too large".to_string(),
        suggestion: None,
    })
}

fn now_unix_seconds() -> Result<i64, ObzError> {
    let duration =
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| ObzError::InvalidArgument {
                code: ErrorCode::ConfigError,
                message: format!("system clock before unix epoch: {e}"),
                suggestion: None,
            })?;
    i64::try_from(duration.as_secs()).map_err(|_| ObzError::InvalidArgument {
        code: ErrorCode::ConfigError,
        message: "system clock is too large".to_string(),
        suggestion: None,
    })
}

fn open_lock(path: &Path) -> Result<RwLock<File>, ObzError> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)
        .map_err(|e| cache_io_error(path, "open cache lock file", &e))?;
    Ok(RwLock::new(file))
}

fn cache_path(cache_dir: &Path, provider_name: &str) -> PathBuf {
    cache_dir.join(format!("{provider_name}.json"))
}

fn lock_path(cache_dir: &Path, provider_name: &str) -> PathBuf {
    cache_dir.join(format!("{provider_name}.lock"))
}

fn cache_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        return PathBuf::from(xdg).join("obz").join("credential-cache");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home)
            .join(".cache")
            .join("obz")
            .join("credential-cache");
    }
    PathBuf::from(".")
        .join(".obz")
        .join("cache")
        .join("credential-cache")
}

fn cache_io_error(path: &Path, action: &str, error: &std::io::Error) -> ObzError {
    ObzError::InvalidArgument {
        code: ErrorCode::ConfigError,
        message: format!("failed to {action} at {}: {error}", path.display()),
        suggestion: None,
    }
}

#[cfg(unix)]
fn set_private_permissions(file: &File, path: &Path) -> Result<(), ObzError> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);
    file.set_permissions(permissions)
        .map_err(|e| cache_io_error(path, "set cache file permissions", &e))
}

#[cfg(unix)]
fn set_dir_permissions(path: &Path) -> Result<(), ObzError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .map_err(|e| cache_io_error(path, "set cache directory permissions", &e))
}

#[cfg(not(unix))]
fn set_dir_permissions(_path: &Path) -> Result<(), ObzError> {
    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_file: &File, _path: &Path) -> Result<(), ObzError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cp_config(args: Vec<String>, cache_ttl: Option<Duration>) -> CredentialProcessConfig {
        CredentialProcessConfig {
            command: "echo".to_string(),
            args,
            timeout: credential_process::DEFAULT_TIMEOUT,
            cache_ttl,
        }
    }

    #[test]
    fn cache_miss_executes_process() {
        let dir = tempfile::tempdir().unwrap();
        let config = cp_config(
            vec![r#"{"version":1,"token":"fresh-token"}"#.to_string()],
            None,
        );

        let output = get_or_refresh_with_cache_dir("vm", &config, false, dir.path()).unwrap();

        assert_eq!(output.auth.get("token"), Some(&"fresh-token".to_string()));
        assert!(cache_path(dir.path(), "vm").is_file());
    }

    #[test]
    fn cache_hit_returns_cached() {
        let dir = tempfile::tempdir().unwrap();
        let first = cp_config(
            vec![r#"{"version":1,"token":"cached-token"}"#.to_string()],
            None,
        );
        let second = cp_config(
            vec![r#"{"version":1,"token":"new-token"}"#.to_string()],
            None,
        );

        let _ = get_or_refresh_with_cache_dir("vm", &first, false, dir.path()).unwrap();
        let output = get_or_refresh_with_cache_dir("vm", &second, false, dir.path()).unwrap();

        assert_eq!(output.auth.get("token"), Some(&"cached-token".to_string()));
    }

    #[test]
    fn cache_expired_triggers_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path(), "vm");
        let now = now_unix_seconds().unwrap();
        let entry = CacheEntry {
            version: CACHE_VERSION,
            cached_at: now.saturating_sub(3600),
            expires_at: Some(now.saturating_sub(1)),
            auth: BTreeMap::from([("token".to_string(), "stale-token".to_string())]),
            headers: BTreeMap::new(),
        };
        write_cache_entry(&path, &entry).unwrap();

        let config = cp_config(
            vec![r#"{"version":1,"token":"fresh-token"}"#.to_string()],
            None,
        );
        let output = get_or_refresh_with_cache_dir("vm", &config, false, dir.path()).unwrap();

        assert_eq!(output.auth.get("token"), Some(&"fresh-token".to_string()));
    }

    #[test]
    fn cache_no_expiration_is_permanent() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path(), "vm");
        let entry = CacheEntry {
            version: CACHE_VERSION,
            cached_at: now_unix_seconds().unwrap(),
            expires_at: None,
            auth: BTreeMap::from([("token".to_string(), "permanent-token".to_string())]),
            headers: BTreeMap::new(),
        };
        write_cache_entry(&path, &entry).unwrap();

        let config = cp_config(
            vec![r#"{"version":1,"token":"new-token"}"#.to_string()],
            None,
        );
        let output = get_or_refresh_with_cache_dir("vm", &config, false, dir.path()).unwrap();

        assert_eq!(
            output.auth.get("token"),
            Some(&"permanent-token".to_string())
        );
    }

    #[test]
    fn cache_corrupt_file_treated_as_miss() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(cache_path(dir.path(), "vm"), "{not-json").unwrap();

        let config = cp_config(
            vec![r#"{"version":1,"token":"fresh-token"}"#.to_string()],
            None,
        );
        let output = get_or_refresh_with_cache_dir("vm", &config, false, dir.path()).unwrap();

        assert_eq!(output.auth.get("token"), Some(&"fresh-token".to_string()));
    }

    #[test]
    fn cache_ttl_from_config() {
        let dir = tempfile::tempdir().unwrap();
        let config = cp_config(
            vec![r#"{"version":1,"token":"ttl-token"}"#.to_string()],
            Some(Duration::from_secs(3600)),
        );

        let _ = get_or_refresh_with_cache_dir("vm", &config, false, dir.path()).unwrap();
        let entry = read_cache_entry(&cache_path(dir.path(), "vm")).unwrap();

        assert!(entry.expires_at.is_some());
        assert_eq!(entry.auth.get("token"), Some(&"ttl-token".to_string()));
    }

    #[test]
    fn protocol_expiration_takes_priority_over_cache_ttl() {
        let dir = tempfile::tempdir().unwrap();
        let future_ts = now_unix_seconds().unwrap() + 600;
        let expiration = jiff::Timestamp::from_second(future_ts)
            .unwrap()
            .strftime("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let json = format!(r#"{{"version":1,"token":"tok","expiration":"{expiration}"}}"#);
        let config = cp_config(vec![json], Some(Duration::from_secs(3600)));

        let _ = get_or_refresh_with_cache_dir("vm", &config, false, dir.path()).unwrap();
        let entry = read_cache_entry(&cache_path(dir.path(), "vm")).unwrap();

        let expires = entry.expires_at.expect("should have expiration");
        let diff = (expires - future_ts).abs();
        assert!(
            diff <= 5,
            "expires_at should match protocol expiration, diff={diff}s"
        );
    }

    #[test]
    fn cache_within_buffer_is_expired() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path(), "vm");
        let now = now_unix_seconds().unwrap();
        let entry = CacheEntry {
            version: CACHE_VERSION,
            cached_at: now - 100,
            expires_at: Some(now + 30),
            auth: BTreeMap::from([("token".to_string(), "stale".to_string())]),
            headers: BTreeMap::new(),
        };
        write_cache_entry(&path, &entry).unwrap();

        let config = cp_config(vec![r#"{"version":1,"token":"fresh"}"#.to_string()], None);
        let output = get_or_refresh_with_cache_dir("vm", &config, false, dir.path()).unwrap();
        assert_eq!(output.auth.get("token"), Some(&"fresh".to_string()));
    }

    #[test]
    fn cache_beyond_buffer_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path(), "vm");
        let now = now_unix_seconds().unwrap();
        let entry = CacheEntry {
            version: CACHE_VERSION,
            cached_at: now - 100,
            expires_at: Some(now + 120),
            auth: BTreeMap::from([("token".to_string(), "cached".to_string())]),
            headers: BTreeMap::new(),
        };
        write_cache_entry(&path, &entry).unwrap();

        let config = cp_config(vec![r#"{"version":1,"token":"fresh"}"#.to_string()], None);
        let output = get_or_refresh_with_cache_dir("vm", &config, false, dir.path()).unwrap();
        assert_eq!(output.auth.get("token"), Some(&"cached".to_string()));
    }

    #[test]
    fn invalidate_removes_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path(), "vm");
        write_cache_entry(
            &path,
            &CacheEntry {
                version: CACHE_VERSION,
                cached_at: now_unix_seconds().unwrap(),
                expires_at: None,
                auth: BTreeMap::new(),
                headers: BTreeMap::new(),
            },
        )
        .unwrap();

        invalidate_in("vm", dir.path());
        assert!(!path.exists());
    }

    #[test]
    fn provider_name_with_path_separator_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let config = cp_config(vec![r#"{"version":1,"token":"tok"}"#.to_string()], None);
        let err = get_or_refresh_with_cache_dir("../evil", &config, false, dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("invalid characters"),
            "expected invalid chars error, got: {err}"
        );
    }
}
