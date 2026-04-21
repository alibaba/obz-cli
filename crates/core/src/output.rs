//! Output formatting pipeline.
//!
//! Inspired by pup's `formatter.rs` — a single generic entry point that
//! handles all output formats for any `T: Serialize`:
//!
//! ```text
//! T: Serialize
//!     → serde_json::to_value()   (preserves struct field order)
//!     → match format {
//!         Json  → pretty print
//!         Table → extract_rows → flatten → auto-detect headers → comfy_table
//!         Csv   → extract_rows → flatten → csv::Writer
//!       }
//! ```
//!
//! Field ordering depends on the data type:
//!
//! - **Typed structs** (`MetricSeries`, `SeriesStats`, etc.): fields appear
//!   in declaration order, guaranteed by serde.
//! - **`BTreeMap` fields** (`labels`, `attributes`): keys are alphabetically
//!   ordered by construction.
//! - **Dynamic `serde_json::Value`** (`ExtensionData.data`): keys preserve
//!   the insertion order from the upstream provider response. This is
//!   deterministic for a given provider but not guaranteed to be alphabetical.
//!
//! This module relies on `serde_json`'s `preserve_order` feature (enabled
//! in the workspace `Cargo.toml`). **Do not remove that feature flag** —
//! without it, `serde_json::Map` falls back to a `BTreeMap` and struct
//! field order will silently revert to alphabetical sorting.
//!
//! Command handlers should call [`format_and_print`] with `&mut io::stdout()`
//! and nothing else.

use std::collections::{HashMap, HashSet};
use std::io::{self, Write};

use comfy_table::presets::UTF8_FULL_CONDENSED;
use comfy_table::{ContentArrangement, Table};
use serde::Serialize;
use serde_json::Map;

/// Maximum number of columns displayed in table output.
const MAX_TABLE_COLUMNS: usize = 12;

/// Maximum string length before truncation in table cells.
const MAX_CELL_LENGTH: usize = 60;

/// Truncation point for long strings (`MAX_CELL_LENGTH` minus 3 for "...").
const CELL_TRUNCATE_AT: usize = MAX_CELL_LENGTH - 3;

/// Output format for CLI results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    /// JSON output (default, optimized for AI Agents).
    Json,
    /// Human-readable table.
    Table,
    /// Comma-separated values.
    Csv,
}

/// Format and write any serializable value to the given writer.
///
/// This is the **single entry point** for all output rendering.
/// Command handlers should call this function and nothing else.
///
/// In production code, pass `&mut io::stdout()` as the writer.
/// In tests, pass `&mut Vec<u8>` to capture output for assertions.
///
/// # Errors
///
/// Returns an IO error if writing fails (e.g., broken pipe).
pub fn format_and_print<T: Serialize>(
    data: &T,
    format: OutputFormat,
    fields: Option<&[String]>,
    truncate: Option<usize>,
    writer: &mut impl Write,
) -> io::Result<()> {
    let mut value = serde_json::to_value(data).map_err(io::Error::other)?;

    if let Some(fields) = fields {
        project_fields(&mut value, fields);
    }
    if let Some(max_chars) = truncate {
        let count = truncate_values(&mut value, max_chars);
        if count > 0 {
            inject_truncated_count(&mut value, count);
        }
    }

    match format {
        OutputFormat::Json => print_json(&value, writer),
        OutputFormat::Table => print_table(&value, writer),
        OutputFormat::Csv => print_csv(&value, writer),
    }
}

// --- JSON output ---

/// Pretty-print a JSON value to the given writer.
fn print_json(value: &serde_json::Value, writer: &mut impl Write) -> io::Result<()> {
    serde_json::to_writer_pretty(&mut *writer, value).map_err(io::Error::other)?;
    writeln!(writer)
}

// --- Table output ---

/// Print a JSON value as a human-readable table.
///
/// Automatically extracts rows from the data, flattens nested objects,
/// and selects the most relevant columns with priority ordering.
fn print_table(value: &serde_json::Value, writer: &mut impl Write) -> io::Result<()> {
    let rows = extract_rows(value);

    if rows.is_empty() {
        return writeln!(writer, "No results found.");
    }

    let (flat_rows, headers) = flatten_and_collect(&rows);

    // Prioritize common fields.
    let priority = [
        "name",
        "timestamp",
        "message",
        "severity",
        "service",
        "source",
        "status",
        "trace_id",
        "span_id",
        "labels",
        "points",
        "stats",
        "result_type",
        "type",
        "description",
        "unit",
    ];
    let final_headers = prioritize_headers(&headers, &priority, MAX_TABLE_COLUMNS);

    let mut table = Table::new();
    table
        .load_preset(UTF8_FULL_CONDENSED)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_header(&final_headers);

    for row in &flat_rows {
        let cells: Vec<String> = final_headers
            .iter()
            .map(|h| format_cell(row.get(h.as_str())))
            .collect();
        table.add_row(cells);
    }

    writeln!(writer, "{table}")?;

    // Footer with row count.
    if flat_rows.len() > 1 {
        writeln!(writer, "\n({} rows)", flat_rows.len())?;
    }

    Ok(())
}

/// Select headers with priority ordering, capped at `max_cols`.
fn prioritize_headers(all: &[String], priority: &[&str], max_cols: usize) -> Vec<String> {
    let all_set: HashSet<&str> = all.iter().map(String::as_str).collect();
    let mut result: Vec<String> = Vec::new();

    // Add priority fields first (if they exist in data).
    for &p in priority {
        if all_set.contains(p) && result.len() < max_cols {
            result.push(p.to_string());
        }
    }

    // Fill remaining slots with other fields.
    for h in all {
        if result.len() >= max_cols {
            break;
        }
        if !result.contains(h) {
            result.push(h.clone());
        }
    }

    result
}

// --- CSV output ---

/// Print a JSON value as CSV using the `csv` crate.
fn print_csv(value: &serde_json::Value, writer: &mut impl Write) -> io::Result<()> {
    let rows = extract_rows(value);
    if rows.is_empty() {
        return Ok(());
    }

    let (flat_rows, headers) = flatten_and_collect(&rows);

    let mut wtr = csv::Writer::from_writer(&mut *writer);
    wtr.write_record(&headers).map_err(io::Error::other)?;

    for row in &flat_rows {
        let cells: Vec<String> = headers
            .iter()
            .map(|h| csv_cell(row.get(h.as_str())))
            .collect();
        wtr.write_record(&cells).map_err(io::Error::other)?;
    }

    wtr.flush()?;
    Ok(())
}

/// Render a JSON value as a plain string for CSV cells.
fn csv_cell(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        Some(other) => other.to_string(),
    }
}

/// Keep only selected fields in row-level output records.
///
/// Automatically discovers object arrays inside the `data` section and
/// applies field projection to each element. Does not require knowing
/// the array key name (`series`, `entries`, `spans`, etc.), so new
/// response types are supported without code changes.
///
/// Supports dot-notation field selectors (e.g. `labels.env` keeps only the
/// `env` key within `labels`). When both a parent (`labels`) and a child
/// (`labels.env`) are specified, the broader parent selector wins.
fn project_fields(value: &mut serde_json::Value, fields: &[String]) {
    let selectors = build_field_selectors(fields);
    if selectors.is_empty() {
        return;
    }

    project_in_value(value, &selectors);
}

fn project_in_value(
    value: &mut serde_json::Value,
    selectors: &HashMap<String, Option<HashSet<String>>>,
) {
    match value {
        // Top-level array (e.g. bare Vec<MetricSeries>)
        serde_json::Value::Array(arr) => {
            for item in arr {
                if item.is_object() {
                    project_row_fields(item, selectors);
                }
            }
        }
        // Response envelope: dig into "data"
        serde_json::Value::Object(map) => {
            if let Some(data) = map.get_mut("data") {
                project_in_data_section(data, selectors);
            }
        }
        _ => {}
    }
}

fn project_in_data_section(
    data: &mut serde_json::Value,
    selectors: &HashMap<String, Option<HashSet<String>>>,
) {
    match data {
        // data is directly an array
        serde_json::Value::Array(arr) => {
            for item in arr {
                if item.is_object() {
                    project_row_fields(item, selectors);
                }
            }
        }
        // data is an object — find all array-of-objects fields and project them
        serde_json::Value::Object(map) => {
            for field_value in map.values_mut() {
                if let serde_json::Value::Array(arr) = field_value {
                    let has_objects = arr.iter().any(serde_json::Value::is_object);
                    if has_objects {
                        for item in arr {
                            if item.is_object() {
                                project_row_fields(item, selectors);
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
}

/// Truncate string values exceeding `max_chars` in row-level data only.
///
/// Like `project_fields`, this automatically discovers object arrays
/// inside the `data` section and truncates string values within them.
/// Structural fields (`result_type`, `trace_id`, `label`, etc.) are
/// never modified.
///
/// Returns the number of string values that were truncated.
fn truncate_values(value: &mut serde_json::Value, max_chars: usize) -> usize {
    truncate_in_value(value, max_chars)
}

fn truncate_in_value(value: &mut serde_json::Value, max_chars: usize) -> usize {
    match value {
        serde_json::Value::Array(arr) => arr
            .iter_mut()
            .map(|item| truncate_value_recursive(item, max_chars))
            .sum(),
        serde_json::Value::Object(map) => {
            if let Some(data) = map.get_mut("data") {
                truncate_in_data_section(data, max_chars)
            } else {
                0
            }
        }
        _ => 0,
    }
}

fn truncate_in_data_section(data: &mut serde_json::Value, max_chars: usize) -> usize {
    match data {
        serde_json::Value::Array(arr) => arr
            .iter_mut()
            .map(|item| truncate_value_recursive(item, max_chars))
            .sum(),
        serde_json::Value::Object(map) => {
            let mut count = 0;
            for field_value in map.values_mut() {
                if let serde_json::Value::Array(arr) = field_value {
                    let has_objects = arr.iter().any(serde_json::Value::is_object);
                    if has_objects {
                        for item in arr {
                            count += truncate_value_recursive(item, max_chars);
                        }
                    }
                }
            }
            count
        }
        _ => 0,
    }
}

fn inject_truncated_count(value: &mut serde_json::Value, count: usize) {
    if let serde_json::Value::Object(map) = value {
        if let Some(serde_json::Value::Object(meta)) = map.get_mut("metadata") {
            meta.insert(
                "truncated_values".to_string(),
                serde_json::Value::Number(count.into()),
            );
        }
    }
}

// --- Shared utilities ---

/// Extract displayable rows from a JSON value.
///
/// Handles common patterns:
/// - Arrays → each element is a row
/// - Objects with a "data" wrapper → unwrap and recurse
/// - Objects with known list fields (series, entries, items) → extract
/// - Single objects → one-row table
fn extract_rows(value: &serde_json::Value) -> Vec<&serde_json::Value> {
    match value {
        serde_json::Value::Array(arr) => arr.iter().collect(),
        serde_json::Value::Object(map) => {
            // Check for known list fields in our response envelope.
            // Response<T>.data is the payload; inside it we look for list fields.
            if let Some(data) = map.get("data") {
                if let serde_json::Value::Object(data_map) = data {
                    // Metric query: data.series
                    if let Some(series) = data_map.get("series") {
                        return extract_rows(series);
                    }
                    // Log search: data.entries
                    if let Some(entries) = data_map.get("entries") {
                        return extract_rows(entries);
                    }
                    // Trace search/get: data.spans
                    if let Some(spans) = data_map.get("spans") {
                        return extract_rows(spans);
                    }
                    // String list: data.items
                    if let Some(items) = data_map.get("items") {
                        return extract_rows(items);
                    }
                    // Extension data: data.data (structured extension results)
                    if let Some(ext_data) = data_map.get("data") {
                        return extract_rows(ext_data);
                    }
                    // Series metadata: data.series (same key)
                    // Scalar: data.scalar
                    if let Some(scalar) = data_map.get("scalar") {
                        // Wrap scalar in a synthetic row for table display.
                        return vec![scalar];
                    }
                    // Metric info: data.info
                    if let Some(info) = data_map.get("info") {
                        if !info.is_null() {
                            return vec![info];
                        }
                        return vec![];
                    }
                }
                // data is an array directly
                return extract_rows(data);
            }
            // Single object → one-row table.
            vec![value]
        }
        _ => vec![],
    }
}

fn build_field_selectors(fields: &[String]) -> HashMap<String, Option<HashSet<String>>> {
    let mut selectors: HashMap<String, Option<HashSet<String>>> = HashMap::new();

    for field in fields {
        let field = field.trim();
        if field.is_empty() {
            continue;
        }

        if let Some((top, nested)) = field.split_once('.') {
            if top.is_empty() || nested.is_empty() {
                continue;
            }
            match selectors.get_mut(top) {
                Some(None) => {}
                Some(Some(nested_fields)) => {
                    nested_fields.insert(nested.to_string());
                }
                None => {
                    let mut nested_fields = HashSet::new();
                    nested_fields.insert(nested.to_string());
                    selectors.insert(top.to_string(), Some(nested_fields));
                }
            }
        } else {
            selectors.insert(field.to_string(), None);
        }
    }

    selectors
}

fn project_row_fields(
    row: &mut serde_json::Value,
    selectors: &HashMap<String, Option<HashSet<String>>>,
) {
    let serde_json::Value::Object(original) = row else {
        return;
    };

    let taken = std::mem::take(original);
    let mut projected = Map::new();

    for (key, value) in taken {
        match selectors.get(&key) {
            Some(None) => {
                projected.insert(key, value);
            }
            Some(Some(nested_fields)) => {
                if let Some(nested_value) = project_nested_value(&value, nested_fields) {
                    projected.insert(key, nested_value);
                }
            }
            None => {}
        }
    }

    *original = projected;
}

fn project_nested_value(
    value: &serde_json::Value,
    nested_fields: &HashSet<String>,
) -> Option<serde_json::Value> {
    let serde_json::Value::Object(map) = value else {
        return None;
    };

    let mut projected = Map::new();
    for (key, nested_value) in map {
        if nested_fields.contains(key) {
            projected.insert(key.clone(), nested_value.clone());
        }
    }

    if projected.is_empty() {
        None
    } else {
        Some(serde_json::Value::Object(projected))
    }
}

fn truncate_value_recursive(value: &mut serde_json::Value, max_chars: usize) -> usize {
    match value {
        serde_json::Value::String(s) => {
            if truncate_string_in_place(s, max_chars) {
                1
            } else {
                0
            }
        }
        serde_json::Value::Array(arr) => arr
            .iter_mut()
            .map(|item| truncate_value_recursive(item, max_chars))
            .sum(),
        serde_json::Value::Object(map) => map
            .values_mut()
            .map(|v| truncate_value_recursive(v, max_chars))
            .sum(),
        serde_json::Value::Null | serde_json::Value::Bool(_) | serde_json::Value::Number(_) => 0,
    }
}

fn truncate_string_in_place(value: &mut String, max_chars: usize) -> bool {
    // Fast path: if byte length is within the limit, char count is too
    // (every char is at least 1 byte). Avoids O(n) char counting for
    // strings that are clearly short enough.
    if value.len() <= max_chars {
        return false;
    }

    let mut char_count = 0;
    let mut boundary = value.len();
    for (idx, _) in value.char_indices() {
        if char_count == max_chars {
            boundary = idx;
            break;
        }
        char_count += 1;
    }

    // All chars fit within max_chars (multi-byte chars made len > max_chars
    // but char count is still within limit).
    if char_count < max_chars {
        return false;
    }

    let original_char_count = char_count + value[boundary..].chars().count();
    let truncated = format!(
        "{}...[truncated, {} chars]",
        &value[..boundary],
        original_char_count
    );
    *value = truncated;
    true
}

/// Flatten rows and collect unique headers preserving insertion order.
fn flatten_and_collect(
    rows: &[&serde_json::Value],
) -> (Vec<Map<String, serde_json::Value>>, Vec<String>) {
    let flat_rows: Vec<Map<String, serde_json::Value>> =
        rows.iter().map(|r| flatten_row(r)).collect();

    let mut header_set = HashSet::new();
    let mut headers: Vec<String> = Vec::new();
    for row in &flat_rows {
        for key in row.keys() {
            if header_set.insert(key.clone()) {
                headers.push(key.clone());
            }
        }
    }

    (flat_rows, headers)
}

/// Flatten a JSON value to dot-notation keys.
///
/// `{"a": {"b": 1}, "c": 2}` → `{"a.b": 1, "c": 2}`
///
/// Arrays are kept as-is (rendered as JSON strings in cells).
/// Uses `serde_json::Map` (backed by `IndexMap` via `preserve_order`)
/// to retain the original struct field ordering.
fn flatten_row(value: &serde_json::Value) -> Map<String, serde_json::Value> {
    let mut out = Map::new();
    flatten_into(value, "", &mut out);
    out
}

fn flatten_into(value: &serde_json::Value, prefix: &str, out: &mut Map<String, serde_json::Value>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                let key = if prefix.is_empty() {
                    k.clone()
                } else {
                    format!("{prefix}.{k}")
                };
                // Don't flatten too deep — stop at depth 2 to keep tables readable.
                if key.matches('.').count() >= 2 {
                    out.insert(key, v.clone());
                } else {
                    flatten_into(v, &key, out);
                }
            }
        }
        _ => {
            if prefix.is_empty() {
                out.insert("value".to_string(), value.clone());
            } else {
                out.insert(prefix.to_string(), value.clone());
            }
        }
    }
}

/// Format a JSON value for table cell display.
///
/// Truncates long strings, compacts arrays, renders nulls as empty.
fn format_cell(value: Option<&serde_json::Value>) -> String {
    match value {
        None | Some(serde_json::Value::Null) => String::new(),
        Some(serde_json::Value::String(s)) => {
            if s.len() > MAX_CELL_LENGTH {
                let mut end = CELL_TRUNCATE_AT;
                while end > 0 && !s.is_char_boundary(end) {
                    end -= 1;
                }
                format!("{}...", &s[..end])
            } else {
                s.clone()
            }
        }
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        Some(serde_json::Value::Array(arr)) => {
            if arr.is_empty() {
                "[]".to_string()
            } else if arr.len() <= 3 {
                // Show small arrays inline.
                let items: Vec<String> = arr.iter().map(|v| format_cell(Some(v))).collect();
                format!("[{}]", items.join(", "))
            } else {
                format!("[{} items]", arr.len())
            }
        }
        Some(serde_json::Value::Object(map)) => {
            if map.is_empty() {
                "{}".to_string()
            } else {
                format!("{{{} fields}}", map.len())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rows_array() {
        let v = serde_json::json!([{"a": 1}, {"a": 2}]);
        assert_eq!(extract_rows(&v).len(), 2);
    }

    #[test]
    fn test_extract_rows_response_envelope() {
        let v = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "matrix",
                "series": [{"name": "cpu"}, {"name": "mem"}]
            }
        });
        assert_eq!(extract_rows(&v).len(), 2);
    }

    #[test]
    fn test_extract_rows_items() {
        let v = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "metric_list",
                "items": ["cpu_usage", "mem_usage"]
            }
        });
        let rows = extract_rows(&v);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_extract_rows_extension_data_string_array() {
        let v = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "services",
                "data": ["cart", "payment", "frontend"]
            }
        });
        let rows = extract_rows(&v);
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn test_extract_rows_extension_data_object_array() {
        let v = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "services",
                "data": [
                    {"name": "cart", "spans": 100},
                    {"name": "payment", "spans": 50}
                ]
            }
        });
        let rows = extract_rows(&v);
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn test_extract_rows_extension_data_single_object() {
        let v = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "config",
                "data": {"key": "value", "count": 42}
            }
        });
        let rows = extract_rows(&v);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn test_flatten_row() {
        let v = serde_json::json!({"a": {"b": 1}, "c": 2});
        let flat = flatten_row(&v);
        assert_eq!(flat["a.b"], serde_json::json!(1));
        assert_eq!(flat["c"], serde_json::json!(2));
    }

    #[test]
    fn test_flatten_row_preserves_field_order() {
        // Use actual struct serialization (not json! macro) to verify that
        // serde's field-declaration order is preserved through flattening.
        use crate::model::metric::{DataPoint, MetricSeries, SeriesStats};
        use std::collections::BTreeMap;

        let series = MetricSeries {
            name: "cpu".to_string(),
            labels: BTreeMap::from([("env".to_string(), "prod".to_string())]),
            points: vec![DataPoint {
                timestamp: 1,
                value: 1.0,
            }],
            stats: Some(SeriesStats {
                min: Some(1.0),
                max: Some(2.0),
                avg: Some(1.5),
                count: 2,
            }),
            extensions: None,
        };
        let value = serde_json::to_value(&series).unwrap();
        let flat = flatten_row(&value);
        let keys: Vec<&String> = flat.keys().collect();
        // Struct field order: name → labels.* → points → stats.* — NOT alphabetical.
        assert_eq!(keys[0], "name");
        assert_eq!(keys[1], "labels.env");
        assert_eq!(keys[2], "points");
        // stats is flattened: min, max, avg, count (struct definition order).
        assert_eq!(keys[3], "stats.min");
        assert_eq!(keys[4], "stats.max");
        assert_eq!(keys[5], "stats.avg");
        assert_eq!(keys[6], "stats.count");
    }

    #[test]
    fn test_format_cell_truncation() {
        let long_str = "a".repeat(100);
        let result = format_cell(Some(&serde_json::Value::String(long_str)));
        assert!(result.ends_with("..."));
        assert!(result.len() <= 63); // 57 chars + "..."
    }

    #[test]
    fn test_format_cell_array() {
        let v = serde_json::json!([1, 2, 3]);
        assert_eq!(format_cell(Some(&v)), "[1, 2, 3]");

        let big = serde_json::json!([1, 2, 3, 4, 5]);
        assert_eq!(format_cell(Some(&big)), "[5 items]");
    }

    #[test]
    fn test_prioritize_headers() {
        let all = vec![
            "z_field".to_string(),
            "name".to_string(),
            "message".to_string(),
            "x_field".to_string(),
        ];
        let priority = ["name", "message", "severity"];
        let result = prioritize_headers(&all, &priority, 3);
        assert_eq!(result, vec!["name", "message", "z_field"]);
    }

    /// End-to-end test: JSON output preserves struct field order.
    ///
    /// Guards against accidental removal of `serde_json`'s `preserve_order`
    /// feature — if that feature is dropped, `serde_json::Map` reverts to
    /// `BTreeMap` and this test will fail with alphabetical key ordering.
    #[test]
    fn test_json_output_preserves_struct_field_order() {
        use crate::model::metric::{DataPoint, MetricSeries, SeriesStats};
        use std::collections::BTreeMap;

        let series = MetricSeries {
            name: "cpu".to_string(),
            labels: BTreeMap::from([("host".to_string(), "web01".to_string())]),
            points: vec![DataPoint {
                timestamp: 1,
                value: 42.0,
            }],
            stats: Some(SeriesStats {
                min: Some(10.0),
                max: Some(90.0),
                avg: Some(50.0),
                count: 5,
            }),
            extensions: None,
        };

        let mut buf = Vec::new();
        format_and_print(&series, OutputFormat::Json, None, None, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();

        // stats fields must appear in struct definition order, not alphabetical.
        let min_pos = output.find("\"min\"").unwrap();
        let max_pos = output.find("\"max\"").unwrap();
        let avg_pos = output.find("\"avg\"").unwrap();
        let count_pos = output.find("\"count\"").unwrap();
        assert!(
            min_pos < max_pos && max_pos < avg_pos && avg_pos < count_pos,
            "stats fields should be in struct order: min, max, avg, count\nActual output:\n{output}"
        );
    }

    #[test]
    fn test_project_fields_basic() {
        let mut value = serde_json::json!({
            "status": "success",
            "metadata": {"provider": "vm"},
            "data": {
                "result_type": "spans",
                "spans": [
                    {"service": "api", "name": "GET /users", "status": "ok"}
                ]
            }
        });

        project_fields(&mut value, &["service".to_string(), "name".to_string()]);

        assert_eq!(
            value["data"]["spans"][0],
            serde_json::json!({"service": "api", "name": "GET /users"})
        );
    }

    #[test]
    fn test_project_fields_dot_notation() {
        let mut value = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "matrix",
                "series": [
                    {"name": "cpu", "labels": {"env": "prod", "host": "web01"}}
                ]
            }
        });

        project_fields(&mut value, &["labels.env".to_string()]);

        assert_eq!(
            value["data"]["series"][0],
            serde_json::json!({"labels": {"env": "prod"}})
        );
    }

    #[test]
    fn test_project_fields_broader_wins() {
        let mut value = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "matrix",
                "series": [
                    {"labels": {"env": "prod", "host": "web01"}}
                ]
            }
        });

        project_fields(
            &mut value,
            &["labels.env".to_string(), "labels".to_string()],
        );

        assert_eq!(
            value["data"]["series"][0],
            serde_json::json!({"labels": {"env": "prod", "host": "web01"}})
        );
    }

    #[test]
    fn test_project_fields_absent_field() {
        let mut value = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "log_entries",
                "entries": [
                    {"timestamp": 1, "message": "hello"}
                ]
            }
        });

        project_fields(&mut value, &["service".to_string()]);

        assert_eq!(value["data"]["entries"][0], serde_json::json!({}));
    }

    #[test]
    fn test_project_fields_with_response_envelope() {
        let mut value = serde_json::json!({
            "status": "success",
            "metadata": {"provider": "vm", "total_count": 1},
            "data": {
                "result_type": "trace_detail",
                "trace_id": "abc123",
                "span_count": 2,
                "service_count": 1,
                "duration_us": 100,
                "services": ["api"],
                "spans": [
                    {"service": "api", "name": "root", "attributes": {"env": "prod"}}
                ]
            }
        });

        project_fields(&mut value, &["service".to_string()]);

        assert_eq!(value["status"], "success");
        assert_eq!(value["metadata"]["provider"], "vm");
        assert_eq!(value["data"]["trace_id"], "abc123");
        assert_eq!(value["data"]["span_count"], 2);
        // String arrays (services) are not projected — only object arrays are.
        assert_eq!(value["data"]["services"], serde_json::json!(["api"]));
        assert_eq!(
            value["data"]["spans"][0],
            serde_json::json!({"service": "api"})
        );
    }

    #[test]
    fn test_truncate_basic() {
        let mut value = serde_json::json!({
            "status": "success",
            "metadata": {"provider": "vm"},
            "data": {
                "result_type": "log_entries",
                "entries": [{"message": "abcdefghij"}]
            }
        });

        truncate_values(&mut value, 5);

        assert_eq!(
            value["data"]["entries"][0]["message"],
            "abcde...[truncated, 10 chars]"
        );
    }

    #[test]
    fn test_truncate_under_limit() {
        let mut value = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "log_entries",
                "entries": [{"message": "short"}]
            }
        });

        truncate_values(&mut value, 10);

        assert_eq!(value["data"]["entries"][0]["message"], "short");
    }

    #[test]
    fn test_truncate_recursive() {
        let mut value = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "spans",
                "spans": [{
                    "attributes": {
                        "http.body": "abcdefghij",
                        "nested": ["klmnopqrstu"]
                    }
                }]
            }
        });

        truncate_values(&mut value, 4);

        assert_eq!(
            value["data"]["spans"][0]["attributes"]["http.body"],
            "abcd...[truncated, 10 chars]"
        );
        assert_eq!(
            value["data"]["spans"][0]["attributes"]["nested"][0],
            "klmn...[truncated, 11 chars]"
        );
    }

    #[test]
    fn test_truncate_non_string() {
        let mut value = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "spans",
                "spans": [{"duration_us": 123, "ok": true, "missing": null}]
            }
        });

        truncate_values(&mut value, 2);

        assert_eq!(value["data"]["spans"][0]["duration_us"], 123);
        assert_eq!(value["data"]["spans"][0]["ok"], true);
        assert_eq!(
            value["data"]["spans"][0]["missing"],
            serde_json::Value::Null
        );
    }

    #[test]
    fn test_truncate_char_boundary() {
        let mut value = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "log_entries",
                "entries": [{"message": "你好世界"}]
            }
        });

        truncate_values(&mut value, 3);

        assert_eq!(
            value["data"]["entries"][0]["message"],
            "你好世...[truncated, 4 chars]"
        );
    }

    #[test]
    fn test_truncate_marker_format() {
        let mut value = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "log_entries",
                "entries": [{"message": "123456789"}]
            }
        });

        truncate_values(&mut value, 2);

        assert_eq!(
            value["data"]["entries"][0]["message"],
            "12...[truncated, 9 chars]"
        );
    }

    #[test]
    fn test_combined_fields_then_truncate() {
        let value = serde_json::json!({
            "status": "success",
            "metadata": {"provider": "dd"},
            "data": {
                "result_type": "log_entries",
                "entries": [{
                    "timestamp": 1,
                    "service": "api",
                    "message": "abcdefghij",
                    "attributes": {"env": "prod"}
                }]
            }
        });

        let mut buf = Vec::new();
        let fields = ["service".to_string(), "message".to_string()];
        format_and_print(&value, OutputFormat::Json, Some(&fields), Some(5), &mut buf).unwrap();
        let output: serde_json::Value = serde_json::from_slice(&buf).unwrap();

        assert_eq!(
            output["data"]["entries"][0],
            serde_json::json!({
                "service": "api",
                "message": "abcde...[truncated, 10 chars]"
            })
        );
        assert_eq!(output["metadata"]["truncated_values"], 1);
    }

    #[test]
    fn test_truncate_no_metadata_when_nothing_truncated() {
        let value = serde_json::json!({
            "status": "success",
            "metadata": {"provider": "vm"},
            "data": {
                "result_type": "log_entries",
                "entries": [{"message": "short"}]
            }
        });

        let mut buf = Vec::new();
        format_and_print(&value, OutputFormat::Json, None, Some(100), &mut buf).unwrap();
        let output: serde_json::Value = serde_json::from_slice(&buf).unwrap();

        assert!(output["metadata"]["truncated_values"].is_null());
    }

    /// CSV output preserves struct field order (no alphabetical sorting).
    #[test]
    fn test_csv_output_preserves_field_order() {
        use crate::model::metric::{DataPoint, MetricSeries, SeriesStats};
        use std::collections::BTreeMap;

        let series = vec![MetricSeries {
            name: "cpu".to_string(),
            labels: BTreeMap::from([("env".to_string(), "prod".to_string())]),
            points: vec![DataPoint {
                timestamp: 1,
                value: 42.0,
            }],
            stats: Some(SeriesStats {
                min: Some(10.0),
                max: Some(90.0),
                avg: Some(50.0),
                count: 5,
            }),
            extensions: None,
        }];

        let mut buf = Vec::new();
        format_and_print(&series, OutputFormat::Csv, None, None, &mut buf).unwrap();
        let output = String::from_utf8(buf).unwrap();

        let header_line = output.lines().next().unwrap();
        let headers: Vec<&str> = header_line.split(',').collect();
        // First header should be "name", not alphabetical "labels.env".
        assert_eq!(headers[0], "name");
        // stats.min should come before stats.count (struct order, not alphabetical).
        let min_idx = headers.iter().position(|h| *h == "stats.min").unwrap();
        let count_idx = headers.iter().position(|h| *h == "stats.count").unwrap();
        assert!(
            min_idx < count_idx,
            "CSV headers should follow struct field order, got: {header_line}"
        );
    }

    /// Dynamic JSON (`ExtensionData`) preserves insertion order.
    #[test]
    fn test_flatten_row_dynamic_json_preserves_insertion_order() {
        // Simulate a realistic Response<ExtensionData> shape with the full
        // envelope (status + data) so extract_rows exercises the real
        // data.data unwrap path.
        let v = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "custom",
                "data": {
                    "zebra": 1,
                    "alpha": 2,
                    "middle": 3
                }
            }
        });

        // extract_rows should unwrap data.data → single object row
        let rows = extract_rows(&v);
        assert_eq!(rows.len(), 1);
        let flat = flatten_row(rows[0]);
        let keys: Vec<&String> = flat.keys().collect();
        // Keys should preserve insertion order (zebra, alpha, middle),
        // NOT alphabetical (alpha, middle, zebra).
        assert_eq!(keys, &["zebra", "alpha", "middle"]);
    }

    #[test]
    fn test_truncate_preserves_result_type() {
        let value = serde_json::json!({
            "status": "success",
            "metadata": {"provider": "vm"},
            "data": {
                "result_type": "log_entries",
                "entries": [{"message": "abcdefghij"}]
            }
        });

        let mut buf = Vec::new();
        format_and_print(&value, OutputFormat::Json, None, Some(3), &mut buf).unwrap();
        let output: serde_json::Value = serde_json::from_slice(&buf).unwrap();

        assert_eq!(output["data"]["result_type"], "log_entries");
        assert_eq!(
            output["data"]["entries"][0]["message"],
            "abc...[truncated, 10 chars]"
        );
    }

    #[test]
    fn test_truncate_preserves_trace_summary() {
        let value = serde_json::json!({
            "status": "success",
            "metadata": {"provider": "vm"},
            "data": {
                "result_type": "trace_detail",
                "trace_id": "abc123def456",
                "span_count": 5,
                "services": ["api-gateway", "payment"],
                "spans": [
                    {"service": "api-gateway", "name": "long-operation-name-that-exceeds"}
                ]
            }
        });

        let mut buf = Vec::new();
        format_and_print(&value, OutputFormat::Json, None, Some(10), &mut buf).unwrap();
        let output: serde_json::Value = serde_json::from_slice(&buf).unwrap();

        assert_eq!(output["data"]["trace_id"], "abc123def456");
        assert_eq!(output["data"]["result_type"], "trace_detail");
        assert_eq!(
            output["data"]["services"],
            serde_json::json!(["api-gateway", "payment"])
        );
        assert!(output["data"]["spans"][0]["name"]
            .as_str()
            .unwrap()
            .contains("[truncated"));
    }

    #[test]
    fn test_extract_rows_spans() {
        let v = serde_json::json!({
            "status": "success",
            "data": {
                "result_type": "spans",
                "spans": [
                    {"service": "api", "name": "GET"},
                    {"service": "db", "name": "SELECT"}
                ]
            }
        });
        assert_eq!(extract_rows(&v).len(), 2);
    }

    #[test]
    fn test_inject_truncated_count_no_metadata() {
        let mut value = serde_json::json!({"data": {"entries": []}});
        inject_truncated_count(&mut value, 5);
        assert!(value.get("metadata").is_none());
    }
}
