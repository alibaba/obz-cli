//! Time expression parser.
//!
//! Parses various time formats used in CLI `--from` and `--to` flags.
//! Delegates to [`parse_datetime`] for the heavy lifting, with a thin
//! normalization layer for Prometheus/Grafana shorthand syntax.
//!
//! # Supported formats
//!
//! | Format | Example | Notes |
//! |--------|---------|-------|
//! | Relative (Prometheus) | `now-1h`, `now-30m`, `-7d` | Normalized then parsed |
//! | Relative (natural) | `1 hour ago`, `+3 days`, `yesterday` | Handled by `parse_datetime` |
//! | RFC3339 / ISO 8601 | `2026-03-24T10:00:00Z` | Handled by `parse_datetime` |
//! | Unix timestamp | `@1740280800` | 10-digit=seconds, 13-digit=milliseconds (auto-converted) |
//! | Now | `now` | Current time |
//!
//! All parsed times are converted to Unix seconds (i64).

use crate::model::error::{ErrorCode, ObzError};

/// Parse a time expression into Unix seconds.
///
/// Supports Prometheus/Grafana shorthand (`now-1h`, `-30m`), Unix
/// timestamps (`@<seconds>` or `@<milliseconds>`), and all formats
/// supported by [`parse_datetime`] (RFC3339, natural language like
/// `yesterday`, `1 hour ago`).
///
/// For `@<digits>` timestamps, 13-digit values are treated as
/// milliseconds and automatically converted to seconds.
///
/// # Arguments
///
/// * `input` — The time expression string.
///
/// # Errors
///
/// Returns [`ObzError::InvalidArgument`] if the input cannot be parsed.
///
/// # Examples
///
/// ```
/// use obz_core::time::parse_time;
///
/// // These all work:
/// let ts = parse_time("now").unwrap();
/// assert!(ts > 0);
///
/// let ts = parse_time("@1740280800").unwrap();
/// assert_eq!(ts, 1740280800);
///
/// // 13-digit millisecond timestamp → auto-converted to seconds
/// let ts = parse_time("@1740280800000").unwrap();
/// assert_eq!(ts, 1740280800);
///
/// let ts = parse_time("2026-03-24T10:00:00Z").unwrap();
/// assert_eq!(ts, 1774346400);
/// ```
pub fn parse_time(input: &str) -> Result<i64, ObzError> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::InvalidTimeRange,
            message: "empty time expression".to_string(),
            suggestion: None,
        });
    }

    // Handle @<unix_timestamp> with 13-digit millisecond auto-conversion.
    if let Some(digits) = trimmed.strip_prefix('@') {
        return parse_unix_timestamp(digits, trimmed);
    }

    let normalized = normalize_shorthand(trimmed);
    let zoned =
        parse_datetime::parse_datetime(&normalized).map_err(|_| ObzError::InvalidArgument {
            code: ErrorCode::InvalidTimeRange,
            message: format!(
                "unrecognized time format: '{trimmed}'. \
                 Supported: now-1h, -1h, RFC3339, @<unix_seconds>, 'yesterday', '1 hour ago'"
            ),
            suggestion: None,
        })?;

    Ok(zoned.timestamp().as_second())
}

/// Parse a `@<digits>` Unix timestamp.
///
/// - 10-digit values are treated as seconds.
/// - 13-digit values are treated as milliseconds and divided by 1000.
/// - Other lengths are parsed as seconds (best effort).
fn parse_unix_timestamp(digits: &str, original: &str) -> Result<i64, ObzError> {
    let ts: i64 = digits.parse().map_err(|_| ObzError::InvalidArgument {
        code: ErrorCode::InvalidTimeRange,
        message: format!(
            "invalid Unix timestamp: '{original}'. Expected @<digits> (10-digit seconds or 13-digit milliseconds)"
        ),
        suggestion: None,
    })?;

    if digits.len() == 13 {
        Ok(ts / 1000)
    } else {
        Ok(ts)
    }
}

/// Parse a query step string (e.g., `15s`, `1m`, `5m`) into seconds.
///
/// Supports Prometheus-style duration suffixes and bare numbers (seconds).
///
/// # Errors
///
/// Returns [`ObzError::InvalidArgument`] if the input is invalid.
///
/// # Examples
///
/// ```
/// use obz_core::time::parse_step;
///
/// assert_eq!(parse_step("15s").unwrap(), 15);
/// assert_eq!(parse_step("1m").unwrap(), 60);
/// assert_eq!(parse_step("5m").unwrap(), 300);
/// assert_eq!(parse_step("1h").unwrap(), 3600);
/// assert_eq!(parse_step("30").unwrap(), 30);
/// ```
pub fn parse_step(input: &str) -> Result<u64, ObzError> {
    let s = input.trim();
    if s.is_empty() {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::InvalidTimeRange,
            message: "step cannot be empty".to_string(),
            suggestion: None,
        });
    }

    // Try as bare number first.
    if let Ok(n) = s.parse::<u64>() {
        if n == 0 {
            return Err(ObzError::InvalidArgument {
                code: ErrorCode::InvalidTimeRange,
                message: "step must be positive".to_string(),
                suggestion: None,
            });
        }
        return Ok(n);
    }

    // Parse "<number><unit>" directly — pure arithmetic, no clock needed.
    let (num, multiplier) = parse_duration_parts(s)?;
    let result = num * multiplier;
    if result == 0 {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::InvalidTimeRange,
            message: "step must be positive".to_string(),
            suggestion: None,
        });
    }
    Ok(result)
}

/// Parse a duration string like `"15s"`, `"1m"`, `"1h"` into `(number, multiplier)`.
///
/// Pure arithmetic — no clock, no allocations.
fn parse_duration_parts(s: &str) -> Result<(u64, u64), ObzError> {
    let (num_str, unit) = s
        .find(|c: char| !c.is_ascii_digit())
        .map(|pos| (&s[..pos], &s[pos..]))
        .ok_or_else(|| ObzError::InvalidArgument {
            code: ErrorCode::InvalidTimeRange,
            message: format!("missing unit in duration: '{s}'. Expected s, m, h, d, or w"),
            suggestion: None,
        })?;

    let num: u64 = num_str.parse().map_err(|_| ObzError::InvalidArgument {
        code: ErrorCode::InvalidTimeRange,
        message: format!("invalid number in duration: '{s}'"),
        suggestion: None,
    })?;

    let multiplier = match unit {
        "s" => 1,
        "m" | "min" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 604800,
        _ => {
            return Err(ObzError::InvalidArgument {
                code: ErrorCode::InvalidTimeRange,
                message: format!("invalid unit '{unit}' in '{s}'. Expected s, m, h, d, or w"),
                suggestion: None,
            });
        }
    };

    Ok((num, multiplier))
}

/// Validate that `from` < `to`.
///
/// # Errors
///
/// Returns [`ObzError::InvalidArgument`] if `from >= to`.
pub fn validate_time_range(from: i64, to: i64) -> Result<(), ObzError> {
    if from >= to {
        return Err(ObzError::InvalidArgument {
            code: ErrorCode::InvalidTimeRange,
            message: format!("--from ({from}) must be before --to ({to})"),
            suggestion: None,
        });
    }
    Ok(())
}

/// Resolve `--from` and `--to` with defaults.
///
/// If `from_str` is `None`, uses `default_from` (e.g., `"now-1h"`).
/// If `to_str` is `None`, uses `"now"`.
///
/// # Errors
///
/// Returns [`ObzError::InvalidArgument`] if either time expression is
/// invalid or if `from >= to`.
pub fn resolve_time_range(
    from_str: Option<&str>,
    to_str: Option<&str>,
    default_from: &str,
) -> Result<(i64, i64), ObzError> {
    let from = parse_time(from_str.unwrap_or(default_from))?;
    let to = parse_time(to_str.unwrap_or("now"))?;
    validate_time_range(from, to)?;
    Ok((from, to))
}

/// Normalize Prometheus/Grafana shorthand to `parse_datetime` format.
///
/// - `now-1h` → `-1 hour`
/// - `now-30m` → `-30 minute`
/// - `-1h` → `-1 hour`
/// - `now+2d` → `+2 day`
/// - Everything else passes through unchanged.
fn normalize_shorthand(input: &str) -> String {
    let s = input.trim();

    // "now" alone — pass through.
    if s == "now" {
        return s.to_string();
    }

    // "now-<num><unit>" → "-<num> <unit>"
    if let Some(rest) = s.strip_prefix("now-") {
        if let Some(expanded) = expand_duration(rest) {
            return format!("-{expanded}");
        }
    }

    // "now+<num><unit>" → "+<num> <unit>"
    if let Some(rest) = s.strip_prefix("now+") {
        if let Some(expanded) = expand_duration(rest) {
            return format!("+{expanded}");
        }
    }

    // "-<num><unit>" shortcut → "-<num> <unit>"
    if let Some(rest) = s.strip_prefix('-') {
        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            if let Some(expanded) = expand_duration(rest) {
                return format!("-{expanded}");
            }
        }
    }

    s.to_string()
}

/// Expand a compact duration like `1h` to `1 hour`.
fn expand_duration(s: &str) -> Option<String> {
    let unit_pos = s.find(|c: char| !c.is_ascii_digit())?;
    let num = &s[..unit_pos];
    if num.is_empty() {
        return None;
    }
    let unit_name = match &s[unit_pos..] {
        "s" => "second",
        "m" | "min" => "minute",
        "h" => "hour",
        "d" => "day",
        "w" => "week",
        _ => return None,
    };
    Some(format!("{num} {unit_name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- normalize_shorthand ---

    #[test]
    fn test_normalize_now() {
        assert_eq!(normalize_shorthand("now"), "now");
    }

    #[test]
    fn test_normalize_relative() {
        assert_eq!(normalize_shorthand("now-1h"), "-1 hour");
        assert_eq!(normalize_shorthand("now-30m"), "-30 minute");
        assert_eq!(normalize_shorthand("now-7d"), "-7 day");
        assert_eq!(normalize_shorthand("now-2w"), "-2 week");
        assert_eq!(normalize_shorthand("now-60s"), "-60 second");
    }

    #[test]
    fn test_normalize_shortcut() {
        assert_eq!(normalize_shorthand("-1h"), "-1 hour");
        assert_eq!(normalize_shorthand("-30m"), "-30 minute");
    }

    #[test]
    fn test_normalize_plus() {
        assert_eq!(normalize_shorthand("now+2d"), "+2 day");
    }

    #[test]
    fn test_normalize_passthrough() {
        assert_eq!(normalize_shorthand("yesterday"), "yesterday");
        assert_eq!(normalize_shorthand("@1740280800"), "@1740280800");
        assert_eq!(
            normalize_shorthand("2026-03-24T10:00:00Z"),
            "2026-03-24T10:00:00Z"
        );
    }

    // --- parse_time ---

    #[test]
    fn test_parse_time_now() {
        let ts = parse_time("now").unwrap();
        let actual_now = jiff::Zoned::now().timestamp().as_second();
        assert!((ts - actual_now).abs() < 2);
    }

    #[test]
    fn test_parse_time_relative() {
        let now = jiff::Zoned::now().timestamp().as_second();
        let ts = parse_time("now-1h").unwrap();
        assert!((ts - (now - 3600)).abs() < 2);

        let ts = parse_time("-30m").unwrap();
        assert!((ts - (now - 1800)).abs() < 2);
    }

    #[test]
    fn test_parse_time_unix() {
        // 10-digit → seconds (pass-through)
        assert_eq!(parse_time("@1740280800").unwrap(), 1740280800);
    }

    #[test]
    fn test_parse_time_unix_milliseconds() {
        // 13-digit → milliseconds, auto-converted to seconds
        assert_eq!(parse_time("@1740280800000").unwrap(), 1740280800);
        assert_eq!(parse_time("@1740280800123").unwrap(), 1740280800);
    }

    #[test]
    fn test_parse_time_unix_invalid() {
        assert!(parse_time("@not_a_number").is_err());
    }

    #[test]
    fn test_parse_time_rfc3339() {
        assert_eq!(parse_time("2026-03-24T10:00:00Z").unwrap(), 1774346400);
    }

    #[test]
    fn test_parse_time_natural() {
        // "yesterday" should be roughly now - 86400
        let ts = parse_time("yesterday").unwrap();
        let now = jiff::Zoned::now().timestamp().as_second();
        assert!((ts - (now - 86400)).abs() < 2);
    }

    #[test]
    fn test_parse_time_invalid() {
        assert!(parse_time("").is_err());
        assert!(parse_time("invalid-garbage").is_err());
    }

    // --- parse_step ---

    #[test]
    fn test_parse_step() {
        assert_eq!(parse_step("15s").unwrap(), 15);
        assert_eq!(parse_step("1m").unwrap(), 60);
        assert_eq!(parse_step("5m").unwrap(), 300);
        assert_eq!(parse_step("1h").unwrap(), 3600);
    }

    #[test]
    fn test_parse_step_bare_number() {
        assert_eq!(parse_step("30").unwrap(), 30);
        assert_eq!(parse_step("60").unwrap(), 60);
    }

    #[test]
    fn test_parse_step_day_week() {
        assert_eq!(parse_step("1d").unwrap(), 86400);
        assert_eq!(parse_step("2w").unwrap(), 1_209_600);
    }

    #[test]
    fn test_parse_step_zero_rejected() {
        assert!(parse_step("0").is_err());
        assert!(parse_step("0s").is_err());
    }

    #[test]
    fn test_parse_step_invalid() {
        assert!(parse_step("").is_err());
        assert!(parse_step("abc").is_err());
    }

    // --- validate/resolve ---

    #[test]
    fn test_validate_time_range_valid() {
        assert!(validate_time_range(100, 200).is_ok());
    }

    #[test]
    fn test_validate_time_range_invalid() {
        assert!(validate_time_range(200, 100).is_err());
        assert!(validate_time_range(100, 100).is_err());
    }

    // --- resolve_time_range ---

    #[test]
    fn test_resolve_time_range_defaults() {
        // When both from and to are None, default_from and "now" are used.
        let (from, to) = resolve_time_range(None, None, "now-1h").unwrap();
        let now = jiff::Zoned::now().timestamp().as_second();
        assert!((from - (now - 3600)).abs() < 2);
        assert!((to - now).abs() < 2);
    }

    #[test]
    fn test_resolve_time_range_explicit_values() {
        // When both from and to are provided, they override defaults.
        let (from, to) = resolve_time_range(Some("@1000"), Some("@2000"), "now-1h").unwrap();
        assert_eq!(from, 1000);
        assert_eq!(to, 2000);
    }

    #[test]
    fn test_resolve_time_range_explicit_from_default_to() {
        // Explicit from, default to ("now").
        let (from, to) = resolve_time_range(Some("@1000"), None, "now-1h").unwrap();
        let now = jiff::Zoned::now().timestamp().as_second();
        assert_eq!(from, 1000);
        assert!((to - now).abs() < 2);
    }

    #[test]
    fn test_resolve_time_range_invalid_range() {
        // from > to should fail validation.
        assert!(resolve_time_range(Some("@2000"), Some("@1000"), "now-1h").is_err());
    }
}
