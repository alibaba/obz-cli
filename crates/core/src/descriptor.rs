//! Extension command and flag descriptors.
//!
//! These pure data structs allow providers to declare their extension
//! commands and provider-specific flags without depending on clap.
//! The obz shell reads these descriptors at startup and registers them
//! as real clap subcommands and arguments, giving full tab-completion
//! and `--help` support.
//!
//! # Design contract
//!
//! Every field in [`FlagDescriptor`] maps directly to a clap argument
//! attribute. obz must be able to losslessly convert a `FlagDescriptor`
//! into a `clap::Arg` without losing any information.

/// The value type of a provider-specific flag.
///
/// Maps to clap's `value_parser` and determines the Rust type used when
/// obz reads the parsed value back out of `ArgMatches`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlagType {
    /// UTF-8 string value (most flags).
    String,
    /// Signed 64-bit integer.
    Int,
    /// Boolean switch (presence = true, `--no-*` = false).
    Bool,
    /// Duration string parsed by the obz time module (e.g. `30s`, `5m`).
    Duration,
}

/// Descriptor for a single provider-specific flag.
///
/// Designed so that obz can convert it to a `clap::Arg` without any
/// additional information. All fields that clap needs are present here.
#[derive(Debug, Clone)]
pub struct FlagDescriptor {
    /// Long flag name without the `--` prefix (e.g. `"round-digits"`).
    /// Must be unique within a command across all providers sharing that command.
    pub name: &'static str,
    /// Value type — determines the clap `value_parser` and `ArgAction`.
    pub flag_type: FlagType,
    /// Whether the flag is required for this provider.
    ///
    /// Always declare `false` at the clap level — enforcement happens at
    /// runtime after the provider is resolved, so other providers that
    /// don't use this flag don't get a clap error.
    pub required: bool,
    /// Default value as a string. `None` means no default.
    pub default: Option<&'static str>,
    /// Short one-line help text shown in `--help`.
    pub description: &'static str,
    /// Whether the flag may be specified multiple times.
    pub repeatable: bool,
    /// Optional single-character short flag (e.g. `Some('q')` for `-q`).
    pub short: Option<char>,
}

/// Descriptor for a provider extension command.
///
/// Extension commands are provider-specific subcommands (e.g.,
/// `metric top-queries`) that are registered into the core command
/// tree alongside the standard commands.
#[derive(Debug, Clone)]
pub struct CommandDescriptor {
    /// Command path relative to the signal group, e.g. `"top-queries"`.
    /// This becomes `obz metric top-queries` when registered under `metric`.
    pub name: &'static str,
    /// Short description shown in `--help`.
    pub description: &'static str,
    /// Flags specific to this extension command.
    pub flags: &'static [FlagDescriptor],
}
