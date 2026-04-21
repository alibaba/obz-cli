//! Configuration file loading and resolution.
//!
//! Reads `config.yaml` from the obz config directory (default
//! `~/.config/obz/`, overridable via `OBZ_CONFIG_DIR`) and resolves
//! provider entries into [`ProviderConfig`] instances.
//!
//! # Config file layout
//!
//! ```yaml
//! providers:
//!   vm:
//!     endpoint: http://localhost:8428
//!     auth:
//!       token: my-bearer-token
//!     headers:
//!       x-custom: value
//!   sls:
//!     endpoint: https://my-proj.cn-hangzhou.log.aliyuncs.com
//!     project: my-proj
//!     auth:
//!       access-key-id: LTAI5t...
//!       access-key-secret: ...
//!
//! defaults:
//!   provider: vm
//!   metric: vm
//!   log: vl
//!   trace: tempo
//! ```

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use obz_core::{ErrorCode, ObzError, ProviderConfig};
use serde::Deserialize;

use crate::credential_process::{self, CredentialProcessConfig};

// ── Public API ──────────────────────────────────────────

/// Parsed contents of `config.yaml`.
#[derive(Debug)]
pub(crate) struct ObzConfig {
    providers: BTreeMap<String, ProviderEntry>,
    defaults: Defaults,
}

/// Default provider selection from `config.yaml`.
#[derive(Debug, Default)]
struct Defaults {
    provider: Option<String>,
    metric: Option<String>,
    log: Option<String>,
    trace: Option<String>,
}

/// A single provider entry from `config.yaml`.
#[derive(Debug)]
struct ProviderEntry {
    values: BTreeMap<String, String>,
    auth: BTreeMap<String, String>,
    headers: BTreeMap<String, String>,
    provider: Option<String>,
    timeout: Option<String>,
    credential_process: Option<CredentialProcessConfig>,
}

/// Load `config.yaml` from `config_dir`.
///
/// Missing files or a missing directory are **not** errors — the returned
/// [`ObzConfig`] will simply have empty providers.
///
/// On Unix, emits a warning to stderr if `config.yaml` has permissions
/// more permissive than `0600`.
///
/// # Errors
///
/// Returns an error only for YAML parse failures or I/O errors.
pub(crate) fn load(config_dir: &Path) -> Result<ObzConfig, ObzError> {
    if !config_dir.is_dir() {
        return Ok(ObzConfig::empty());
    }

    let path = config_dir.join("config.yaml");
    if !path.is_file() {
        return Ok(ObzConfig::empty());
    }

    check_permissions(&path);

    let content = std::fs::read_to_string(&path).map_err(|e| ObzError::InvalidArgument {
        code: ErrorCode::ConfigError,
        message: format!("failed to read {}: {e}", path.display()),
        suggestion: None,
    })?;

    if content.trim().is_empty() {
        return Ok(ObzConfig::empty());
    }

    let raw: RawConfigFile =
        serde_yaml::from_str(&content).map_err(|e| ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: format!("failed to parse {}: {e}", path.display()),
            suggestion: None,
        })?;

    let defaults = Defaults {
        provider: raw.defaults.provider,
        metric: raw.defaults.metric,
        log: raw.defaults.log,
        trace: raw.defaults.trace,
    };

    let mut providers = BTreeMap::new();
    for (name, raw_values) in raw.providers {
        providers.insert(name.clone(), parse_provider_entry(&name, raw_values)?);
    }

    Ok(ObzConfig {
        providers,
        defaults,
    })
}

impl ObzConfig {
    pub(crate) fn empty() -> Self {
        Self {
            providers: BTreeMap::new(),
            defaults: Defaults::default(),
        }
    }

    pub(crate) fn provider_names(&self) -> Vec<&str> {
        self.providers.keys().map(String::as_str).collect()
    }

    /// Resolve without variable reference expansion (for tests).
    #[cfg(test)]
    pub(crate) fn resolve(&self, name: &str) -> Result<Option<ProviderConfig>, ObzError> {
        self.resolve_with_dir(name, None)
    }

    /// Resolve a `-p` name into a [`ProviderConfig`], resolving variable
    /// references relative to the given `config_dir`.
    pub(crate) fn resolve_with_dir(
        &self,
        name: &str,
        config_dir: Option<&Path>,
    ) -> Result<Option<ProviderConfig>, ObzError> {
        let Some(entry) = self.providers.get(name) else {
            return Ok(None);
        };

        let mut config = ProviderConfig::new();

        for (k, v) in &entry.values {
            let resolved =
                resolve_if_needed(v, config_dir).map_err(|e| wrap_resolve_error(e, name, k))?;
            config.set(k, &resolved);
        }

        for (k, v) in &entry.auth {
            let resolved = resolve_if_needed(v, config_dir)
                .map_err(|e| wrap_resolve_error(e, name, &format!("auth.{k}")))?;
            if !resolved.is_empty() {
                config.set_auth(k, &resolved);
            }
        }

        for (k, v) in &entry.headers {
            let resolved = resolve_if_needed(v, config_dir)
                .map_err(|e| wrap_resolve_error(e, name, &format!("headers.{k}")))?;
            config.set_header(k, &resolved);
        }

        if let Some(timeout_str) = &entry.timeout {
            let secs =
                obz_core::time::parse_step(timeout_str).map_err(|e| ObzError::InvalidArgument {
                    code: ErrorCode::ConfigError,
                    message: format!(
                        "invalid timeout \"{timeout_str}\" for provider \"{name}\": {e}"
                    ),
                    suggestion: None,
                })?;
            config.set_timeout(Duration::from_secs(secs));
        }

        Ok(Some(config))
    }

    /// Determine the provider type for a given `-p` name.
    pub(crate) fn provider_type<'a>(&'a self, name: &'a str) -> &'a str {
        self.providers
            .get(name)
            .and_then(|entry| entry.provider.as_deref())
            .unwrap_or(name)
    }

    pub(crate) fn credential_process(&self, name: &str) -> Option<&CredentialProcessConfig> {
        self.providers
            .get(name)
            .and_then(|e| e.credential_process.as_ref())
    }

    pub(crate) fn default_provider(&self, signal: &str) -> Option<&str> {
        let per_signal = match signal {
            "metric" => self.defaults.metric.as_deref(),
            "log" => self.defaults.log.as_deref(),
            "trace" => self.defaults.trace.as_deref(),
            _ => return None,
        };
        per_signal.or(self.defaults.provider.as_deref())
    }

    pub(crate) fn default_metric(&self) -> Option<&str> {
        self.defaults
            .metric
            .as_deref()
            .or(self.defaults.provider.as_deref())
    }

    pub(crate) fn default_log(&self) -> Option<&str> {
        self.defaults
            .log
            .as_deref()
            .or(self.defaults.provider.as_deref())
    }

    pub(crate) fn default_trace(&self) -> Option<&str> {
        self.defaults
            .trace
            .as_deref()
            .or(self.defaults.provider.as_deref())
    }

    /// Return provider identifiers configured in `config.yaml`.
    ///
    /// Used by `obz skill install` for config-aware filtering.
    pub(crate) fn configured_provider_types(&self) -> std::collections::BTreeSet<String> {
        let mut result = std::collections::BTreeSet::new();
        for name in self.providers.keys() {
            result.insert(name.clone());
            result.insert(self.provider_type(name).to_string());
        }
        result
    }
}

fn resolve_if_needed(value: &str, config_dir: Option<&Path>) -> Result<String, ObzError> {
    match config_dir {
        Some(dir) => crate::resolve::resolve_value(value, dir),
        None => Ok(value.to_string()),
    }
}

fn wrap_resolve_error(err: ObzError, provider_name: &str, key: &str) -> ObzError {
    match err {
        ObzError::InvalidArgument {
            code,
            message,
            suggestion,
        } => ObzError::InvalidArgument {
            code,
            message: format!("providers.{provider_name}.{key}: {message}"),
            suggestion,
        },
        other => other,
    }
}

// ── YAML Deserialization Models ─────────────────────────

#[derive(Deserialize, Default)]
struct RawConfigFile {
    #[serde(default)]
    providers: BTreeMap<String, BTreeMap<String, serde_yaml::Value>>,
    #[serde(default)]
    defaults: RawDefaults,
}

#[derive(Deserialize, Default)]
struct RawDefaults {
    provider: Option<String>,
    metric: Option<String>,
    log: Option<String>,
    trace: Option<String>,
}

// ── Parsing ────────────────────────────────────────────

/// Top-level keys that were previously valid but must now be placed under `auth:`.
const LEGACY_AUTH_KEYS: &[&str] = &[
    "token",
    "username",
    "password",
    "access-key-id",
    "access-key-secret",
    "api-key",
    "app-key",
];

fn parse_provider_entry(
    name: &str,
    raw: BTreeMap<String, serde_yaml::Value>,
) -> Result<ProviderEntry, ObzError> {
    let mut values = BTreeMap::new();
    let mut auth = BTreeMap::new();
    let mut headers = BTreeMap::new();
    let mut provider = None;
    let mut timeout = None;
    let mut credential_process = None;

    for (k, v) in raw {
        match k.as_str() {
            "provider" => {
                provider = yaml_value_to_string(&v);
            }
            "timeout" => {
                timeout = yaml_value_to_string(&v);
            }
            "credential" => {
                eprintln!(
                    "[warn] providers.{name}: 'credential' is no longer supported. \
                     Move credentials directly under 'auth:'"
                );
            }
            "auth" => {
                if let serde_yaml::Value::Mapping(map) = v {
                    for (mk, mv) in map {
                        if let serde_yaml::Value::String(key) = mk {
                            if key == "credential-process" {
                                credential_process = Some(parse_credential_process(name, &mv)?);
                            } else if let Some(val) = yaml_value_to_string(&mv) {
                                auth.insert(key, val);
                            }
                        }
                    }
                }
            }
            "headers" => {
                if let serde_yaml::Value::Mapping(map) = v {
                    for (mk, mv) in map {
                        if let serde_yaml::Value::String(key) = mk {
                            if let Some(val) = yaml_value_to_string(&mv) {
                                headers.insert(key.to_ascii_lowercase(), val);
                            }
                        }
                    }
                }
            }
            _ => {
                if LEGACY_AUTH_KEYS.contains(&k.as_str()) {
                    eprintln!(
                        "[warn] providers.{name}.{k}: this key will be ignored. \
                         Move to: providers.{name}.auth.{k}"
                    );
                    continue;
                }
                if let Some(string_val) = yaml_value_to_string(&v) {
                    values.insert(k, string_val);
                }
            }
        }
    }

    Ok(ProviderEntry {
        values,
        auth,
        headers,
        provider,
        timeout,
        credential_process,
    })
}

fn parse_credential_process(
    name: &str,
    value: &serde_yaml::Value,
) -> Result<CredentialProcessConfig, ObzError> {
    let serde_yaml::Value::Mapping(map) = value else {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: format!("providers.{name}.auth.credential-process: expected a mapping"),
            suggestion: None,
        });
    };

    let command = map
        .get(serde_yaml::Value::String("command".into()))
        .and_then(yaml_value_to_string);

    let Some(command) = command else {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: format!(
                "providers.{name}.auth.credential-process: missing required 'command' field"
            ),
            suggestion: None,
        });
    };

    let args = map
        .get(serde_yaml::Value::String("args".into()))
        .and_then(|v| {
            if let serde_yaml::Value::Sequence(seq) = v {
                Some(seq.iter().filter_map(yaml_value_to_string).collect())
            } else {
                None
            }
        })
        .unwrap_or_default();

    let timeout = match map
        .get(serde_yaml::Value::String("timeout".into()))
        .and_then(yaml_value_to_string)
    {
        Some(s) => {
            let secs = obz_core::time::parse_step(&s).map_err(|e| ObzError::InvalidArgument {
                code: ErrorCode::ConfigError,
                message: format!(
                    "providers.{name}.auth.credential-process.timeout: \
                     invalid duration \"{s}\": {e}"
                ),
                suggestion: None,
            })?;
            Duration::from_secs(secs)
        }
        None => credential_process::DEFAULT_TIMEOUT,
    };

    let cache_ttl = match map
        .get(serde_yaml::Value::String("cache-ttl".into()))
        .and_then(yaml_value_to_string)
    {
        Some(s) => {
            let secs = obz_core::time::parse_step(&s).map_err(|e| ObzError::InvalidArgument {
                code: ErrorCode::ConfigError,
                message: format!(
                    "providers.{name}.auth.credential-process.cache-ttl: \
                     invalid duration \"{s}\": {e}"
                ),
                suggestion: None,
            })?;
            Some(Duration::from_secs(secs))
        }
        None => None,
    };

    Ok(CredentialProcessConfig {
        command,
        args,
        timeout,
        cache_ttl,
    })
}

fn yaml_value_to_string(v: &serde_yaml::Value) -> Option<String> {
    match v {
        serde_yaml::Value::String(s) => Some(s.clone()),
        serde_yaml::Value::Bool(b) => Some(b.to_string()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        serde_yaml::Value::Null => None,
        other => Some(serde_yaml::to_string(other).unwrap_or_default()),
    }
}

fn check_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use std::sync::OnceLock;

        // `load()` may be called multiple times during a single invocation
        // (pre-parse for default provider, then again in dispatch). Guard
        // with a OnceLock so the warning is emitted at most once.
        static WARNED: OnceLock<()> = OnceLock::new();

        if let Ok(metadata) = std::fs::metadata(path) {
            let mode = metadata.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                WARNED.get_or_init(|| {
                    eprintln!(
                        "[warn] {} has permissions {:04o}, recommend 0600",
                        path.display(),
                        mode,
                    );
                });
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
}

// ── Tests ───────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup_dir(config_yaml: Option<&str>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        if let Some(content) = config_yaml {
            fs::write(dir.path().join("config.yaml"), content).unwrap();
        }
        dir
    }

    #[test]
    fn load_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nonexistent");
        let cfg = load(&missing).unwrap();
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn load_config_with_values_only() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.providers.len(), 1);
        assert_eq!(
            cfg.providers["vm"].values["endpoint"],
            "http://localhost:8428"
        );
        assert!(cfg.providers["vm"].auth.is_empty());
        assert!(cfg.providers["vm"].headers.is_empty());
    }

    #[test]
    fn load_config_with_auth_block() {
        let dir = setup_dir(Some(
            r#"
providers:
  sls:
    endpoint: http://sls.example.com
    project: test
    auth:
      access-key-id: LTAI5t
      access-key-secret: mysecret
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.providers["sls"].auth["access-key-id"], "LTAI5t");
        assert_eq!(cfg.providers["sls"].auth["access-key-secret"], "mysecret");
        assert_eq!(cfg.providers["sls"].values["project"], "test");
    }

    #[test]
    fn load_config_with_headers_block() {
        let dir = setup_dir(Some(
            r#"
providers:
  mimir:
    endpoint: http://localhost:9009
    headers:
      X-Scope-OrgID: my-tenant
      X-Custom: value
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let headers = &cfg.providers["mimir"].headers;
        assert_eq!(headers["x-scope-orgid"], "my-tenant");
        assert_eq!(headers["x-custom"], "value");
    }

    #[test]
    fn resolve_builds_provider_config() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    auth:
      token: my-token
    headers:
      x-custom: val
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg.resolve("vm").unwrap().unwrap();

        assert_eq!(pc.get("endpoint"), Some("http://localhost:8428"));
        assert_eq!(pc.bearer_token(), Some("my-token".to_string()));
        assert_eq!(
            pc.custom_headers().get("x-custom"),
            Some(&"val".to_string())
        );
    }

    #[test]
    fn resolve_named_instance_with_provider_field() {
        let dir = setup_dir(Some(
            r#"
providers:
  app-logs:
    provider: sls
    endpoint: http://sls.example.com
    project: test
    logstore: app-logs
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg.resolve("app-logs").unwrap().unwrap();
        assert_eq!(pc.get("endpoint"), Some("http://sls.example.com"));
        assert_eq!(pc.get("logstore"), Some("app-logs"));
        assert_eq!(pc.get("provider"), None);
        assert_eq!(cfg.provider_type("app-logs"), "sls");
    }

    #[test]
    fn resolve_not_found() {
        let dir = setup_dir(Some("providers: {}"));
        let cfg = load(dir.path()).unwrap();
        assert!(cfg.resolve("unknown").unwrap().is_none());
    }

    #[test]
    fn resolve_with_auth() {
        let dir = setup_dir(Some(
            r#"
providers:
  dd:
    endpoint: https://api.datadoghq.com
    auth:
      api-key: myapikey
      app-key: myappkey
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg.resolve("dd").unwrap().unwrap();
        assert_eq!(pc.get("endpoint"), Some("https://api.datadoghq.com"));
        assert_eq!(pc.auth_get("api-key"), Some("myapikey"));
        assert_eq!(pc.auth_get("app-key"), Some("myappkey"));
    }

    #[test]
    fn resolve_timeout_parsed_as_duration() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    timeout: 30s
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg.resolve("vm").unwrap().unwrap();
        assert_eq!(pc.timeout(), Some(Duration::from_secs(30)));
    }

    #[test]
    fn resolve_timeout_invalid_returns_error() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    timeout: invalid
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let err = cfg.resolve("vm").unwrap_err();
        assert!(err.to_string().contains("timeout"));
    }

    #[test]
    fn provider_type_from_explicit_field() {
        let dir = setup_dir(Some(
            r#"
providers:
  my-logs:
    provider: sls
    endpoint: http://sls.example.com
  vm:
    endpoint: http://localhost:8428
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.provider_type("my-logs"), "sls");
        assert_eq!(cfg.provider_type("vm"), "vm");
        assert_eq!(cfg.provider_type("dd"), "dd");
    }

    #[test]
    fn invalid_yaml() {
        let dir = setup_dir(Some("providers: [invalid"));
        let err = load(dir.path()).unwrap_err();
        assert!(err.to_string().contains("parse"), "error: {err}");
        match &err {
            ObzError::InvalidArgument { code, .. } => {
                assert_eq!(*code, ErrorCode::ConfigError);
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn numeric_values_converted_to_string() {
        let dir = setup_dir(Some(
            r#"
providers:
  test:
    endpoint: http://localhost
    port: 8428
    enabled: true
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg.resolve("test").unwrap().unwrap();
        assert_eq!(pc.get("port"), Some("8428"));
        assert_eq!(pc.get("enabled"), Some("true"));
    }

    #[test]
    fn resolve_null_value_skipped() {
        let dir = setup_dir(Some(
            r#"
providers:
  test:
    endpoint: http://localhost
    auth:
      token:
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg.resolve("test").unwrap().unwrap();
        assert_eq!(pc.get("endpoint"), Some("http://localhost"));
        assert_eq!(pc.bearer_token(), None);
    }

    #[test]
    fn load_empty_config_yaml() {
        let dir = setup_dir(Some(""));
        let cfg = load(dir.path()).unwrap();
        assert!(cfg.providers.is_empty());
    }

    #[test]
    fn provider_type_without_config_entry() {
        let cfg = ObzConfig::empty();
        assert_eq!(cfg.provider_type("vm"), "vm");
        assert_eq!(cfg.provider_type("sls"), "sls");
    }

    // ── Defaults tests ─────────────────────────────────────

    #[test]
    fn default_provider_per_signal() {
        let dir = setup_dir(Some(
            r#"
defaults:
  metric: vm
  log: vl
  trace: tempo

providers:
  vm:
    endpoint: http://localhost:8428
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.default_provider("metric"), Some("vm"));
        assert_eq!(cfg.default_provider("log"), Some("vl"));
        assert_eq!(cfg.default_provider("trace"), Some("tempo"));
    }

    #[test]
    fn default_provider_global_fallback() {
        let dir = setup_dir(Some(
            r#"
defaults:
  provider: sls

providers:
  sls:
    endpoint: http://sls.example.com
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.default_provider("metric"), Some("sls"));
        assert_eq!(cfg.default_provider("log"), Some("sls"));
        assert_eq!(cfg.default_provider("trace"), Some("sls"));
    }

    #[test]
    fn default_provider_per_signal_overrides_global() {
        let dir = setup_dir(Some(
            r#"
defaults:
  provider: sls
  metric: vm

providers: {}
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.default_provider("metric"), Some("vm"));
        assert_eq!(cfg.default_provider("log"), Some("sls"));
        assert_eq!(cfg.default_provider("trace"), Some("sls"));
    }

    #[test]
    fn default_provider_none_when_not_configured() {
        let cfg = ObzConfig::empty();
        assert_eq!(cfg.default_provider("metric"), None);
        assert_eq!(cfg.default_provider("log"), None);
        assert_eq!(cfg.default_provider("trace"), None);
    }

    #[test]
    fn default_provider_unknown_signal() {
        let dir = setup_dir(Some(
            r#"
defaults:
  provider: sls
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.default_provider("completions"), None);
        assert_eq!(cfg.default_provider("provider"), None);
    }

    #[test]
    fn default_provider_partial_signals() {
        let dir = setup_dir(Some(
            r#"
defaults:
  metric: vm
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.default_provider("metric"), Some("vm"));
        assert_eq!(cfg.default_provider("log"), None);
        assert_eq!(cfg.default_provider("trace"), None);
    }

    #[test]
    fn defaults_section_missing() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.default_provider("metric"), None);
        assert_eq!(cfg.default_provider("log"), None);
    }

    #[test]
    fn provider_names_return_sorted_keys() {
        let dir = setup_dir(Some(
            r#"
providers:
  zeta:
    endpoint: http://zeta
  alpha:
    endpoint: http://alpha
  beta:
    endpoint: http://beta
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.provider_names(), vec!["alpha", "beta", "zeta"]);
    }

    #[test]
    fn default_accessors_follow_signal_then_global_fallback() {
        let dir = setup_dir(Some(
            r#"
defaults:
  provider: shared
  metric: vm
  trace: tempo
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert_eq!(cfg.default_metric(), Some("vm"));
        assert_eq!(cfg.default_log(), Some("shared"));
        assert_eq!(cfg.default_trace(), Some("tempo"));
    }

    #[test]
    fn header_keys_lowercased_on_load() {
        let dir = setup_dir(Some(
            r#"
providers:
  mimir:
    endpoint: http://localhost:9009
    headers:
      X-Scope-OrgID: tenant-1
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg.resolve("mimir").unwrap().unwrap();
        assert_eq!(
            pc.custom_headers().get("x-scope-orgid"),
            Some(&"tenant-1".to_string())
        );
    }

    #[test]
    fn auth_and_headers_not_in_values() {
        let dir = setup_dir(Some(
            r#"
providers:
  test:
    endpoint: http://localhost
    auth:
      token: secret
    headers:
      x-custom: val
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg.resolve("test").unwrap().unwrap();
        assert_eq!(pc.get("auth"), None);
        assert_eq!(pc.get("headers"), None);
        assert_eq!(pc.get("endpoint"), Some("http://localhost"));
    }

    #[test]
    fn provider_field_not_in_values() {
        let dir = setup_dir(Some(
            r#"
providers:
  my-sls:
    provider: sls
    endpoint: http://sls.example.com
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg.resolve("my-sls").unwrap().unwrap();
        assert_eq!(pc.get("provider"), None);
        assert_eq!(pc.get("endpoint"), Some("http://sls.example.com"));
    }

    #[test]
    fn timeout_field_not_in_values() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    timeout: 1m
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg.resolve("vm").unwrap().unwrap();
        assert_eq!(pc.get("timeout"), None);
        assert_eq!(pc.timeout(), Some(Duration::from_secs(60)));
    }

    #[test]
    fn credential_process_cache_ttl_parsed() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    auth:
      credential-process:
        command: echo
        cache-ttl: 5m
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let cp = cfg.credential_process("vm").unwrap();
        assert_eq!(cp.cache_ttl, Some(Duration::from_secs(300)));
        assert_eq!(cp.timeout, credential_process::DEFAULT_TIMEOUT);
    }

    // ── Variable reference integration tests ───────────────

    #[test]
    fn resolve_with_dir_resolves_env_in_auth() {
        use std::sync::Mutex;
        static M: Mutex<()> = Mutex::new(());
        let _lock = M.lock().unwrap();

        let prev = std::env::var("OBZ_TEST_RESOLVE_TOKEN").ok();
        std::env::set_var("OBZ_TEST_RESOLVE_TOKEN", "resolved-secret");

        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    auth:
      token: ${env:OBZ_TEST_RESOLVE_TOKEN}
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg
            .resolve_with_dir("vm", Some(dir.path()))
            .unwrap()
            .unwrap();
        assert_eq!(pc.bearer_token(), Some("resolved-secret".to_string()));

        match prev {
            Some(v) => std::env::set_var("OBZ_TEST_RESOLVE_TOKEN", v),
            None => std::env::remove_var("OBZ_TEST_RESOLVE_TOKEN"),
        }
    }

    #[test]
    fn resolve_with_dir_resolves_file_in_auth() {
        let dir = setup_dir(Some(
            r#"
providers:
  sls:
    endpoint: http://sls.example.com
    auth:
      access-key-id: ${file:ak.txt}
"#,
        ));
        std::fs::write(dir.path().join("ak.txt"), "LTAI5t-from-file\n").unwrap();
        let cfg = load(dir.path()).unwrap();
        let pc = cfg
            .resolve_with_dir("sls", Some(dir.path()))
            .unwrap()
            .unwrap();
        assert_eq!(pc.auth_get("access-key-id"), Some("LTAI5t-from-file"));
    }

    #[test]
    fn resolve_with_dir_env_optional_empty_in_auth_is_absent() {
        use std::sync::Mutex;
        static M: Mutex<()> = Mutex::new(());
        let _lock = M.lock().unwrap();

        std::env::remove_var("OBZ_TEST_OPT_UNSET_39281");

        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    auth:
      token: ${env?:OBZ_TEST_OPT_UNSET_39281}
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let pc = cfg
            .resolve_with_dir("vm", Some(dir.path()))
            .unwrap()
            .unwrap();
        assert_eq!(pc.bearer_token(), None);
    }

    #[test]
    fn resolve_with_dir_error_includes_key_path() {
        use std::sync::Mutex;
        static M: Mutex<()> = Mutex::new(());
        let _lock = M.lock().unwrap();

        std::env::remove_var("OBZ_TEST_MISSING_VAR_82910");

        let dir = setup_dir(Some(
            r#"
providers:
  dd:
    endpoint: https://api.datadoghq.com
    auth:
      api-key: ${env:OBZ_TEST_MISSING_VAR_82910}
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let err = cfg.resolve_with_dir("dd", Some(dir.path())).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("providers.dd.auth.api-key"),
            "error should contain key path: {msg}"
        );
        assert!(
            msg.contains("not set"),
            "error should mention var not set: {msg}"
        );
    }

    // ── credential-process config parsing tests ────────────

    #[test]
    fn credential_process_parsed_from_auth_block() {
        let dir = setup_dir(Some(
            r#"
providers:
  es-prod:
    provider: es
    endpoint: https://es.example.com:9200
    auth:
      username: elastic
      credential-process:
        command: vault
        args: ["kv", "get", "-format=json", "secret/es-prod"]
        timeout: 10s
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let cp = cfg.credential_process("es-prod");
        assert!(cp.is_some(), "credential-process should be parsed");
        let cp = cp.unwrap();
        assert_eq!(cp.command, "vault");
        assert_eq!(cp.args, vec!["kv", "get", "-format=json", "secret/es-prod"]);
        assert_eq!(cp.timeout, Duration::from_secs(10));

        let pc = cfg.resolve("es-prod").unwrap().unwrap();
        assert_eq!(pc.auth_get("username"), Some("elastic"));
    }

    #[test]
    fn credential_process_without_args_or_timeout() {
        let dir = setup_dir(Some(
            r#"
providers:
  prom-vault:
    provider: prom
    endpoint: https://prom.example.com
    auth:
      credential-process:
        command: /usr/local/bin/get-prom-token
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let cp = cfg.credential_process("prom-vault").unwrap();
        assert_eq!(cp.command, "/usr/local/bin/get-prom-token");
        assert!(cp.args.is_empty());
        assert_eq!(cp.timeout, Duration::from_secs(30));
    }

    #[test]
    fn credential_process_not_present_returns_none() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    auth:
      token: plain-token
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert!(cfg.credential_process("vm").is_none());
    }

    #[test]
    fn credential_process_no_auth_block_returns_none() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        assert!(cfg.credential_process("vm").is_none());
    }

    #[test]
    fn credential_process_coexists_with_inline_auth() {
        let dir = setup_dir(Some(
            r#"
providers:
  dd:
    endpoint: https://api.datadoghq.com
    auth:
      api-key: fallback-key
      credential-process:
        command: get-dd-creds
        args: ["--env", "prod"]
"#,
        ));
        let cfg = load(dir.path()).unwrap();
        let cp = cfg.credential_process("dd").unwrap();
        assert_eq!(cp.command, "get-dd-creds");
        assert_eq!(cp.args, vec!["--env", "prod"]);

        let pc = cfg.resolve("dd").unwrap().unwrap();
        assert_eq!(pc.auth_get("api-key"), Some("fallback-key"));
    }

    #[test]
    fn credential_process_invalid_timeout_errors() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    auth:
      credential-process:
        command: get-token
        timeout: invalid
"#,
        ));
        let err = load(dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("timeout") && msg.contains("invalid"),
            "error should mention invalid timeout: {msg}"
        );
    }

    #[test]
    fn credential_process_missing_command_errors() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    auth:
      credential-process:
        args: ["--flag"]
"#,
        ));
        let err = load(dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("command"),
            "error should mention missing command: {msg}"
        );
    }

    #[test]
    fn credential_process_not_a_mapping_errors() {
        let dir = setup_dir(Some(
            r#"
providers:
  vm:
    endpoint: http://localhost:8428
    auth:
      credential-process: just-a-string
"#,
        ));
        let err = load(dir.path()).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("expected a mapping"),
            "error should mention mapping: {msg}"
        );
    }
}
