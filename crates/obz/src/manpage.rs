//! Man page generation for the obz CLI.
//!
//! Generates ROFF man pages from the runtime clap command tree using
//! `clap_mangen`. Because obz has a dynamic command tree (providers register
//! extension commands and flags at startup), man pages cannot be generated
//! at build time in `build.rs`. Instead, we expose a hidden subcommand
//! (`generate-man-pages`) that runs after providers are registered, producing
//! the complete set of man pages.
//!
//! ## Naming convention
//!
//! Follows the standard `<bin>-<subcommand>.1` pattern:
//!
//! ```text
//! obz.1                   # top-level
//! obz-metric.1            # signal group
//! obz-metric-query.1      # leaf command
//! obz-trace-services.1    # extension command
//! ```

use std::fs;
use std::io::Write;
use std::path::Path;

use clap::Command;

/// Generate man pages for `cmd` and all its subcommands, writing ROFF
/// files into `out_dir`.
///
/// Each page is named `<prefix>.1` where prefix follows the
/// `obz-metric-query` convention (parent names joined by hyphens).
pub(crate) fn generate_all(cmd: &Command, out_dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(out_dir)?;
    generate_recursive(cmd, out_dir, cmd.get_name())
}

/// Recursively generate man pages for a command and its subcommands.
fn generate_recursive(cmd: &Command, out_dir: &Path, prefix: &str) -> std::io::Result<()> {
    // Use Man::title() to set the full prefixed name (e.g. "obz-metric-query")
    // so that clap_mangen emits the correct .TH header and NAME section.
    // Without this, subcommands would show bare names like "query" instead of
    // "obz-metric-query".
    let man = clap_mangen::Man::new(cmd.clone()).title(prefix);
    let mut buf: Vec<u8> = Vec::new();
    man.render(&mut buf)?;

    let filename = format!("{prefix}.1");
    let path = out_dir.join(&filename);
    let mut file = fs::File::create(&path)?;
    file.write_all(&buf)?;

    // Recurse into subcommands.
    // Skip "help" (clap auto-generates it) and hidden commands (internal
    // dev tools like generate-man-pages that end users should not see).
    for sub in cmd.get_subcommands() {
        if sub.get_name() == "help" || sub.is_hide_set() {
            continue;
        }
        let child_prefix = format!("{prefix}-{}", sub.get_name());
        generate_recursive(sub, out_dir, &child_prefix)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verify that `generate_all` produces the expected set of man page files
    /// for a small command tree.
    #[test]
    fn test_generate_creates_expected_files() {
        let cmd = Command::new("obz")
            .subcommand(
                Command::new("metric")
                    .subcommand(Command::new("query"))
                    .subcommand(Command::new("list")),
            )
            .subcommand(Command::new("log").subcommand(Command::new("search")))
            .subcommand(Command::new("hidden-dev-tool").hide(true));

        let dir = tempfile::tempdir().unwrap();
        generate_all(&cmd, dir.path()).unwrap();

        let expected = [
            "obz.1",
            "obz-metric.1",
            "obz-metric-query.1",
            "obz-metric-list.1",
            "obz-log.1",
            "obz-log-search.1",
        ];

        for name in &expected {
            let path = dir.path().join(name);
            assert!(path.exists(), "missing man page: {name}");

            let content = fs::read_to_string(&path).unwrap();
            assert!(
                content.contains(".TH"),
                "{name} should contain ROFF .TH directive"
            );
        }

        // Verify .TH titles use full prefixed names, not bare subcommand names.
        let metric_page = fs::read_to_string(dir.path().join("obz-metric.1")).unwrap();
        assert!(
            metric_page.contains(".TH obz-metric"),
            "obz-metric.1 should have .TH obz-metric, got: {}",
            metric_page
                .lines()
                .find(|l| l.contains(".TH"))
                .unwrap_or("(none)")
        );

        let query_page = fs::read_to_string(dir.path().join("obz-metric-query.1")).unwrap();
        assert!(
            query_page.contains(".TH obz-metric-query"),
            "obz-metric-query.1 should have .TH obz-metric-query, got: {}",
            query_page
                .lines()
                .find(|l| l.contains(".TH"))
                .unwrap_or("(none)")
        );

        // "help" subcommand should NOT generate a man page.
        assert!(
            !dir.path().join("obz-help.1").exists(),
            "should not generate man page for 'help'"
        );

        // Hidden commands should NOT generate man pages.
        assert!(
            !dir.path().join("obz-hidden-dev-tool.1").exists(),
            "should not generate man page for hidden commands"
        );
    }

    /// Verify that the full obz command tree (with all providers registered)
    /// generates a non-trivial number of man pages.
    #[test]
    fn test_generate_with_full_registry() {
        use obz_core::registry::ProviderRegistry;

        let mut registry = ProviderRegistry::new();
        obz_providers::register_all(&mut registry);
        let cmd = crate::cli::build_cli(&registry, None);

        let dir = tempfile::tempdir().unwrap();
        generate_all(&cmd, dir.path()).unwrap();

        let pages: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "1"))
            .collect();

        // At minimum: obz.1, obz-metric.1, obz-metric-query.1, obz-metric-list.1,
        // obz-metric-info.1, obz-metric-labels.1, obz-metric-label-values.1,
        // obz-metric-series.1, obz-log.1, obz-log-search.1, obz-trace.1,
        // obz-trace-search.1, obz-trace-get.1, obz-completions.1, obz-provider.1,
        // obz-provider-list.1, plus extension commands.
        // Hidden commands (generate-man-pages) are excluded.
        assert!(
            pages.len() >= 15,
            "expected at least 15 man pages, got {}",
            pages.len()
        );

        // Spot-check: obz-metric-query.1 should mention --query.
        let query_page = fs::read_to_string(dir.path().join("obz-metric-query.1")).unwrap();
        assert!(
            query_page.contains("query"),
            "obz-metric-query.1 should reference the --query flag"
        );
    }
}
