//! Standard command identifiers for provider `command_flags` declarations.
//!
//! Providers use [`StandardCommand`] variants as keys in their `command_flags`
//! slice to associate extension flags with specific commands.
//!
//! # Example
//!
//! ```rust
//! use obz_core::cmd_path::StandardCommand;
//!
//! static MY_FLAGS: &[(StandardCommand, &[obz_core::FlagDescriptor])] = &[
//!     (StandardCommand::MetricQuery, &[]),
//!     (StandardCommand::LogSearch, &[]),
//! ];
//! ```

// Generate the `StandardCommand` enum and its `signal()`/`subcommand()`
// methods from a single declaration table.
//
// Each entry maps a variant name to its `signal / subcommand` pair.
// Adding a new command is a one-liner — the enum definition **and** both
// accessor methods are kept in sync automatically.
macro_rules! standard_commands {
    ( $( $(#[$attr:meta])* $variant:ident => $signal:literal / $subcommand:literal ),* $(,)? ) => {
        /// Identifies a standard (core) command in the obz CLI.
        ///
        /// Used as the key in
        /// [`ProviderMeta::command_flags`](crate::registry::ProviderMeta::command_flags)
        /// to associate provider-specific flags with specific commands.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum StandardCommand {
            $( $(#[$attr])* $variant, )*
        }

        impl StandardCommand {
            /// The signal group this command belongs to
            /// (e.g. `"metric"`, `"log"`, `"trace"`).
            pub const fn signal(self) -> &'static str {
                match self {
                    $( Self::$variant => $signal, )*
                }
            }

            /// The subcommand name within its signal group
            /// (e.g. `"query"`, `"search"`).
            pub const fn subcommand(self) -> &'static str {
                match self {
                    $( Self::$variant => $subcommand, )*
                }
            }
        }
    };
}

standard_commands! {
    /// `obz metric query`
    MetricQuery      => "metric" / "query",
    /// `obz metric list`
    MetricList       => "metric" / "list",
    /// `obz metric info`
    MetricInfo       => "metric" / "info",
    /// `obz metric labels`
    MetricLabels     => "metric" / "labels",
    /// `obz metric label-values`
    MetricLabelValues => "metric" / "label-values",
    /// `obz metric series`
    MetricSeries     => "metric" / "series",
    /// `obz log search`
    LogSearch        => "log"    / "search",
    /// `obz trace search`
    TraceSearch      => "trace"  / "search",
    /// `obz trace get`
    TraceGet         => "trace"  / "get",
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_and_subcommand_round_trip() {
        assert_eq!(StandardCommand::MetricQuery.signal(), "metric");
        assert_eq!(StandardCommand::MetricQuery.subcommand(), "query");
        assert_eq!(StandardCommand::MetricList.signal(), "metric");
        assert_eq!(StandardCommand::MetricList.subcommand(), "list");
        assert_eq!(StandardCommand::MetricInfo.signal(), "metric");
        assert_eq!(StandardCommand::MetricInfo.subcommand(), "info");
        assert_eq!(StandardCommand::MetricLabels.signal(), "metric");
        assert_eq!(StandardCommand::MetricLabels.subcommand(), "labels");
        assert_eq!(StandardCommand::MetricLabelValues.signal(), "metric");
        assert_eq!(
            StandardCommand::MetricLabelValues.subcommand(),
            "label-values"
        );
        assert_eq!(StandardCommand::MetricSeries.signal(), "metric");
        assert_eq!(StandardCommand::MetricSeries.subcommand(), "series");
        assert_eq!(StandardCommand::LogSearch.signal(), "log");
        assert_eq!(StandardCommand::LogSearch.subcommand(), "search");
        assert_eq!(StandardCommand::TraceSearch.signal(), "trace");
        assert_eq!(StandardCommand::TraceSearch.subcommand(), "search");
        assert_eq!(StandardCommand::TraceGet.signal(), "trace");
        assert_eq!(StandardCommand::TraceGet.subcommand(), "get");
    }
}
