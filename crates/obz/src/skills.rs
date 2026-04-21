//! Skill management for AI coding assistants.
//!
//! Skills are Markdown files (`SKILL.md`) that teach AI coding agents how
//! to use obz.  Each provider gets its own skill describing the supported
//! commands, query language, authentication, and examples.
//!
//! The skills are embedded into the binary at compile time via `include_str!`
//! and installed to a user-specified directory with `obz skills install`.
//!
//! # Design
//!
//! - Skills follow the open [SKILL.md standard](https://agentskills.io)
//!   (YAML frontmatter with `name` + `description`, Markdown body).
//! - Each `SkillEntry` references a provider's **canonical name** (from
//!   `ProviderMeta::name`).  Config-aware install resolves config keys
//!   to canonical names via `ProviderRegistry::get()`.
//! - The `obz-core` skill (provider = `None`) is always installed.

use std::io::Write;
use std::path::Path;

use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::{ContentArrangement, Table};
use obz_core::output::OutputFormat;
use obz_core::registry::ProviderRegistry;
use obz_core::{ErrorCode, ObzError};

use crate::config;

// ── Skill registry ─────────────────────────────────────

/// A single installable skill.
pub(crate) struct SkillEntry {
    /// Skill identifier, used as subdirectory name (e.g. `"obz-vm"`).
    pub name: &'static str,
    /// Short description for `obz skills list` output.
    pub description: &'static str,
    /// Canonical provider name this skill covers (e.g. `"victoriametrics"`).
    /// `None` for the core skill, which is always installed by default.
    pub provider: Option<&'static str>,
    /// `SKILL.md` content, embedded at compile time.
    pub content: &'static str,
}

/// All available skills, compiled into the binary.
pub(crate) static SKILLS: &[SkillEntry] = &[
    SkillEntry {
        name: "obz-core",
        description: "Core commands, config, and output modes",
        provider: None,
        content: include_str!("../../../skills/obz-core/SKILL.md"),
    },
    SkillEntry {
        name: "obz-vm",
        description: "Metric queries via MetricsQL",
        provider: Some("victoriametrics"),
        content: include_str!("../../../skills/obz-vm/SKILL.md"),
    },
    SkillEntry {
        name: "obz-vl",
        description: "Log search via LogsQL",
        provider: Some("victorialogs"),
        content: include_str!("../../../skills/obz-vl/SKILL.md"),
    },
    SkillEntry {
        name: "obz-vt",
        description: "Trace search via Jaeger API",
        provider: Some("victoriatraces"),
        content: include_str!("../../../skills/obz-vt/SKILL.md"),
    },
    SkillEntry {
        name: "obz-sls",
        description: "Metric (PromQL), log, and trace search",
        provider: Some("sls"),
        content: include_str!("../../../skills/obz-sls/SKILL.md"),
    },
    SkillEntry {
        name: "obz-dd",
        description: "Metric (DQL), log, and trace search",
        provider: Some("datadog"),
        content: include_str!("../../../skills/obz-dd/SKILL.md"),
    },
    SkillEntry {
        name: "obz-prometheus",
        description: "Metric queries via PromQL",
        provider: Some("prometheus"),
        content: include_str!("../../../skills/obz-prometheus/SKILL.md"),
    },
    SkillEntry {
        name: "obz-jaeger",
        description: "Trace search via Jaeger API",
        provider: Some("jaeger"),
        content: include_str!("../../../skills/obz-jaeger/SKILL.md"),
    },
    SkillEntry {
        name: "obz-opensearch",
        description: "Log and trace search via OpenSearch DSL",
        provider: Some("opensearch"),
        content: include_str!("../../../skills/obz-opensearch/SKILL.md"),
    },
    SkillEntry {
        name: "obz-elasticsearch",
        description: "Log and trace search via Elasticsearch Query DSL",
        provider: Some("elasticsearch"),
        content: include_str!("../../../skills/obz-elasticsearch/SKILL.md"),
    },
    SkillEntry {
        name: "obz-mimir",
        description: "Metric queries via PromQL",
        provider: Some("mimir"),
        content: include_str!("../../../skills/obz-mimir/SKILL.md"),
    },
    SkillEntry {
        name: "obz-loki",
        description: "Log search via LogQL",
        provider: Some("loki"),
        content: include_str!("../../../skills/obz-loki/SKILL.md"),
    },
    SkillEntry {
        name: "obz-tempo",
        description: "Trace search via TraceQL",
        provider: Some("tempo"),
        content: include_str!("../../../skills/obz-tempo/SKILL.md"),
    },
];

// ── Public interface (called from dispatch) ────────────

/// Look up the skill name for a canonical provider type.
///
/// Returns the skill name (e.g. `"obz-vm"`) for a canonical provider
/// name (e.g. `"victoriametrics"`).  Returns `None` if no skill
/// covers the given provider type.
pub(crate) fn skill_name_for_provider(canonical_name: &str) -> Option<&'static str> {
    SKILLS
        .iter()
        .find(|e| e.provider == Some(canonical_name))
        .map(|e| e.name)
}

/// Build a hint string suggesting the user run `obz skills show <skill>`
/// for the given canonical provider name.
///
/// Returns `None` if no matching skill exists.
pub(crate) fn skill_hint(canonical_name: &str) -> Option<String> {
    skill_name_for_provider(canonical_name)
        .map(|name| format!("Run 'obz skills show {name}' for full configuration guide"))
}

pub(crate) fn list(registry: &ProviderRegistry, output: OutputFormat) -> Result<(), ObzError> {
    match output {
        OutputFormat::Table => {
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            writeln!(out, "{}", render_skill_table(registry)).map_err(io_err)?;
            writeln!(
                out,
                "\n{} skills available. Use 'obz skills show <name>' for details.",
                SKILLS.len()
            )
            .map_err(io_err)?;
            Ok(())
        }
        _ => list_json(),
    }
}

/// Print the SKILL.md content for the given skill names.
pub(crate) fn show(names: &[String]) -> Result<(), ObzError> {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    show_to_writer(names, &mut out)
}

fn show_to_writer(names: &[String], out: &mut impl Write) -> Result<(), ObzError> {
    for (i, name) in names.iter().enumerate() {
        let entry = find_skill(name)?;
        if i > 0 {
            writeln!(out, "\n---\n").map_err(io_err)?;
        }
        write!(out, "{}", entry.content).map_err(io_err)?;
    }
    Ok(())
}

/// Install skills to `dir`.
///
/// If `names` is non-empty, only those skills are installed (regardless of
/// config).  If `all` is true, every skill is installed.  Otherwise, only
/// the core skill and skills matching configured providers are installed.
pub(crate) fn install(
    dir: &str,
    names: &[String],
    all: bool,
    config_dir: &Path,
    registry: &ProviderRegistry,
) -> Result<(), ObzError> {
    let to_install: Vec<&SkillEntry> = if !names.is_empty() {
        let mut seen = std::collections::BTreeSet::new();
        let mut entries = Vec::new();
        for name in names {
            if !seen.insert(name.as_str()) {
                continue;
            }
            entries.push(find_skill(name)?);
        }
        entries
    } else if all {
        SKILLS.iter().collect()
    } else {
        let configured = configured_providers(config_dir, registry);
        SKILLS
            .iter()
            .filter(|e| match e.provider {
                None => true,
                Some(p) => configured.contains(p),
            })
            .collect()
    };

    if to_install.is_empty() {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::InvalidFlag,
            message: "no skills to install — run `obz skills list` to see available skills"
                .to_string(),
            suggestion: None,
        });
    }

    let dir_path = expand_tilde(dir);
    let mut installed = Vec::new();

    for entry in &to_install {
        let skill_dir = dir_path.join(entry.name);
        std::fs::create_dir_all(&skill_dir).map_err(|e| ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: format!("failed to create {}: {e}", skill_dir.display()),
            suggestion: None,
        })?;
        let file_path = skill_dir.join("SKILL.md");
        std::fs::write(&file_path, entry.content).map_err(|e| ObzError::InvalidArgument {
            code: ErrorCode::ConfigError,
            message: format!("failed to write {}: {e}", file_path.display()),
            suggestion: None,
        })?;
        installed.push(entry.name);
    }

    let config_hint = if names.is_empty() && !all && installed.len() == 1 {
        Some(
            "No providers found in config. Use --all or specify skill names.\n\
             Example: obz skills install --dir <path> --all",
        )
    } else {
        None
    };

    print_install_json(&installed, &dir_path, config_hint)?;

    Ok(())
}

// ── Helpers ────────────────────────────────────────────

fn find_skill(name: &str) -> Result<&SkillEntry, ObzError> {
    SKILLS
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| ObzError::InvalidArgument {
            code: ErrorCode::InvalidFlag,
            message: format!(
                "unknown skill '{name}' — run `obz skills list` to see available skills"
            ),
            suggestion: None,
        })
}

fn io_err(e: impl std::fmt::Display) -> ObzError {
    ObzError::InvalidArgument {
        code: ErrorCode::ConfigError,
        message: format!("write error: {e}"),
        suggestion: None,
    }
}

/// Expand a leading `~` to the user's home directory.
fn expand_tilde(path: &str) -> std::path::PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return std::path::PathBuf::from(home).join(rest);
        }
    }
    std::path::PathBuf::from(path)
}

/// Read `config.yaml` and return canonical provider type names.
fn configured_providers(
    config_dir: &Path,
    registry: &ProviderRegistry,
) -> std::collections::BTreeSet<String> {
    let Ok(obz_config) = config::load(config_dir) else {
        return std::collections::BTreeSet::new();
    };
    obz_config
        .configured_provider_types()
        .into_iter()
        .map(|key| {
            registry
                .get(&key)
                .map(|meta| meta.name.to_string())
                .unwrap_or(key)
        })
        .collect()
}

// ── Output formatting ──────────────────────────────────

fn render_skill_table(registry: &ProviderRegistry) -> String {
    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(["Skill", "Provider", "Description"]);

    for entry in SKILLS {
        let provider_display = match entry.provider {
            Some(canonical) => match registry.get(canonical) {
                Ok(meta) => meta.display_name,
                Err(_) => canonical,
            },
            None => "(core)",
        };
        table.add_row([entry.name, provider_display, entry.description]);
    }

    table.to_string()
}

fn list_json() -> Result<(), ObzError> {
    let entries: Vec<serde_json::Value> = SKILLS
        .iter()
        .map(|e| {
            serde_json::json!({
                "name": e.name,
                "description": e.description,
                "provider": e.provider,
            })
        })
        .collect();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer_pretty(&mut out, &entries).map_err(io_err)?;
    writeln!(out).map_err(io_err)?;
    Ok(())
}

fn print_install_json(installed: &[&str], dir: &Path, hint: Option<&str>) -> Result<(), ObzError> {
    let mut obj = serde_json::json!({
        "installed": installed,
        "directory": dir.display().to_string(),
    });
    if let Some(h) = hint {
        obj["hint"] = serde_json::Value::String(h.replace('\n', " "));
    }
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    serde_json::to_writer_pretty(&mut out, &obj).map_err(io_err)?;
    writeln!(out).map_err(io_err)?;
    Ok(())
}

// ── Tests ──────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_entries_have_valid_names() {
        for entry in SKILLS {
            assert!(!entry.name.is_empty(), "empty name");
            assert!(entry.name.len() <= 64, "name too long: {}", entry.name);
            assert!(
                entry
                    .name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "invalid chars in name: {}",
                entry.name
            );
        }
    }

    #[test]
    fn all_entries_have_descriptions() {
        for entry in SKILLS {
            assert!(
                !entry.description.is_empty(),
                "empty description for {}",
                entry.name
            );
            assert!(
                entry.description.len() <= 1024,
                "description too long for {}",
                entry.name
            );
        }
    }

    #[test]
    fn all_entries_have_content() {
        for entry in SKILLS {
            assert!(
                !entry.content.is_empty(),
                "empty content for {}",
                entry.name
            );
            assert!(
                entry.content.starts_with("---"),
                "missing frontmatter in {}",
                entry.name
            );
        }
    }

    #[test]
    fn no_duplicate_names() {
        let mut names: Vec<&str> = SKILLS.iter().map(|e| e.name).collect();
        names.sort();
        for w in names.windows(2) {
            assert_ne!(w[0], w[1], "duplicate skill name: {}", w[0]);
        }
    }

    #[test]
    fn core_skill_has_no_provider() {
        let core = SKILLS.iter().find(|e| e.name == "obz-core").unwrap();
        assert!(core.provider.is_none());
    }

    #[test]
    fn provider_skills_have_provider() {
        for entry in SKILLS {
            if entry.name != "obz-core" {
                assert!(
                    entry.provider.is_some(),
                    "non-core skill {} has no provider",
                    entry.name
                );
            }
        }
    }

    #[test]
    fn skill_count() {
        assert_eq!(
            SKILLS.len(),
            13,
            "expected 13 skills (1 core + 12 providers)"
        );
    }

    #[test]
    fn expand_tilde_basic() {
        assert_eq!(
            expand_tilde("/tmp/skills"),
            std::path::PathBuf::from("/tmp/skills")
        );
        assert_eq!(
            expand_tilde("./skills"),
            std::path::PathBuf::from("./skills")
        );
    }

    #[test]
    fn expand_tilde_with_home() {
        let home = std::env::var("HOME").unwrap_or_default();
        if !home.is_empty() {
            let result = expand_tilde("~/my-skills");
            assert_eq!(result, std::path::PathBuf::from(&home).join("my-skills"));
        }
    }

    #[test]
    fn install_all_creates_files() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();

        let registry = ProviderRegistry::new();
        let result = install(
            dir.path().to_str().unwrap(),
            &[],
            true,
            config_dir.path(),
            &registry,
        );
        assert!(result.is_ok());

        for entry in SKILLS {
            let skill_file = dir.path().join(entry.name).join("SKILL.md");
            assert!(skill_file.exists(), "missing {}", skill_file.display());
            let content = std::fs::read_to_string(&skill_file).unwrap();
            assert_eq!(content, entry.content);
        }
    }

    #[test]
    fn install_by_name() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::new();

        let result = install(
            dir.path().to_str().unwrap(),
            &["obz-vm".to_string(), "obz-sls".to_string()],
            false,
            config_dir.path(),
            &registry,
        );
        assert!(result.is_ok());

        assert!(dir.path().join("obz-vm/SKILL.md").exists());
        assert!(dir.path().join("obz-sls/SKILL.md").exists());
        assert!(!dir.path().join("obz-dd/SKILL.md").exists());
    }

    #[test]
    fn install_deduplicates_names() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::new();

        let result = install(
            dir.path().to_str().unwrap(),
            &[
                "obz-vm".to_string(),
                "obz-vm".to_string(),
                "obz-sls".to_string(),
            ],
            false,
            config_dir.path(),
            &registry,
        );
        assert!(result.is_ok());
        assert!(dir.path().join("obz-vm/SKILL.md").exists());
        assert!(dir.path().join("obz-sls/SKILL.md").exists());
    }

    #[test]
    fn install_unknown_name_errors() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::new();

        let result = install(
            dir.path().to_str().unwrap(),
            &["nonexistent".to_string()],
            false,
            config_dir.path(),
            &registry,
        );
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ObzError::InvalidArgument { message, .. } => {
                assert!(message.contains("nonexistent"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn install_no_config_installs_core_only() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::new();

        let result = install(
            dir.path().to_str().unwrap(),
            &[],
            false,
            config_dir.path(),
            &registry,
        );
        assert!(result.is_ok());

        assert!(dir.path().join("obz-core/SKILL.md").exists());
        assert!(!dir.path().join("obz-vm/SKILL.md").exists());
    }

    #[test]
    fn configured_provider_types_empty_dir() {
        let config_dir = tempfile::tempdir().unwrap();
        let registry = ProviderRegistry::new();
        let types = configured_providers(config_dir.path(), &registry);
        assert!(types.is_empty());
    }

    #[test]
    fn find_skill_returns_entry() {
        let entry = find_skill("obz-core").unwrap();
        assert_eq!(entry.name, "obz-core");
        assert!(entry.provider.is_none());
    }

    #[test]
    fn find_skill_unknown_errors() {
        let result = find_skill("nonexistent");
        assert!(result.is_err());
        match result.err().unwrap() {
            ObzError::InvalidArgument { message, .. } => {
                assert!(message.contains("nonexistent"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn show_single_skill() {
        let mut buf = Vec::new();
        show_to_writer(&["obz-core".to_string()], &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        assert!(
            output.starts_with("---"),
            "should start with YAML frontmatter"
        );
        assert!(output.contains("obz-core"));
    }

    #[test]
    fn show_multiple_skills_separated() {
        let mut buf = Vec::new();
        show_to_writer(&["obz-core".to_string(), "obz-vm".to_string()], &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();
        let parts: Vec<&str> = output.split("\n\n---\n\n").collect();
        assert_eq!(parts.len(), 2, "expected two skills separated by ---");
        assert!(parts[0].contains("obz-core"));
        assert!(parts[1].contains("obz-vm"));
    }

    #[test]
    fn show_unknown_skill_errors() {
        let mut buf = Vec::new();
        let result = show_to_writer(&["nonexistent".to_string()], &mut buf);
        assert!(result.is_err());
        let err = result.unwrap_err();
        match err {
            ObzError::InvalidArgument { message, .. } => {
                assert!(message.contains("nonexistent"));
            }
            other => panic!("expected InvalidArgument, got {other:?}"),
        }
    }

    #[test]
    fn skill_name_for_known_provider() {
        assert_eq!(skill_name_for_provider("victoriametrics"), Some("obz-vm"));
        assert_eq!(skill_name_for_provider("sls"), Some("obz-sls"));
        assert_eq!(skill_name_for_provider("datadog"), Some("obz-dd"));
    }

    #[test]
    fn skill_name_for_unknown_provider() {
        assert_eq!(skill_name_for_provider("nonexistent"), None);
    }

    #[test]
    fn skill_hint_returns_command() {
        let hint = skill_hint("victoriametrics").unwrap();
        assert!(hint.contains("obz skills show obz-vm"));
    }

    #[test]
    fn skill_hint_none_for_unknown() {
        assert!(skill_hint("nonexistent").is_none());
    }

    #[test]
    fn render_skill_table_contains_all_skills() {
        let registry = ProviderRegistry::new();
        let output = render_skill_table(&registry);
        for entry in SKILLS {
            assert!(
                output.contains(entry.name),
                "table missing skill: {}",
                entry.name
            );
        }
    }

    #[test]
    fn render_skill_table_core_shows_core_marker() {
        let registry = ProviderRegistry::new();
        let output = render_skill_table(&registry);
        assert!(output.contains("(core)"));
    }

    #[test]
    fn render_skill_table_empty_registry_falls_back_to_canonical() {
        let registry = ProviderRegistry::new();
        let output = render_skill_table(&registry);
        assert!(
            output.contains("victoriametrics"),
            "empty registry should fall back to canonical name"
        );
    }

    #[test]
    fn render_skill_table_with_registered_provider_shows_display_name() {
        use obz_core::registry::{BuiltProvider, SupportedCommands};

        let mut registry = ProviderRegistry::new();
        registry.register(obz_core::registry::ProviderMeta {
            name: "victoriametrics",
            display_name: "VictoriaMetrics",
            aliases: &["vm", "victoriametrics"],
            supported_commands: SupportedCommands::default(),
            build: |_| {
                Ok(BuiltProvider {
                    name: "victoriametrics",
                    metric_query_language: None,
                    log_query_language: None,
                    metric: None,
                    log: None,
                    trace: None,
                    extension: None,
                })
            },
            check: None,
            command_flags: &[],
            extension_commands: &[],
        });
        let output = render_skill_table(&registry);
        assert!(
            output.contains("VictoriaMetrics"),
            "registered provider should show display_name"
        );
        assert!(
            !output.contains("victoriametrics"),
            "canonical name should not appear when display_name is available"
        );
    }
}
