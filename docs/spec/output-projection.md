# obz Output Projection Spec: `--fields` and `--truncate`

> Design spec for two new global output flags that give agents and users
> fine-grained control over what appears in CLI output.
>
> **Status**: Implemented (v0.1.0).

---

## Overview

Two new flags for all query commands (`metric`, `log`, `trace`, and
extension commands):

| Flag | Purpose | Dimension |
|------|---------|-----------|
| `--fields` | Select which fields to keep per record | Columns |
| `--truncate` | Cap the character length of string values | Cell content |

Both are **output-layer concerns** — they act on the serialized
`serde_json::Value` after provider execution but before format rendering.
Providers are unaware of these flags.

---

## 1. `--fields` — Field Projection

### 1.1 Syntax

```
--fields <field1>,<field2>,...
```

Comma-separated list of field names. Supports dot notation for nested
fields (up to 2 levels).

### 1.2 Scope

`--fields` operates on **row-level data only**:

- `metric query`: each `MetricSeries` in `data.series`
- `log search`: each `LogEntry` in `data.entries`
- `trace search/get`: each `Span` in `data.spans`
- Extension commands: each element in `data.data` (if array of objects)

The response **envelope** (`status`, `metadata`, `data.result_type`) and
**trace_detail summary** (`trace_id`, `span_count`, `service_count`,
`duration_us`, `services`) are **never** affected by `--fields`.

### 1.3 Dot Notation Rules

| Input | Behavior | Example |
|-------|----------|---------|
| `name` | Keep entire top-level field | `"name": "cpu"` kept as-is |
| `labels` | Keep entire `labels` object | All labels kept |
| `labels.env` | Keep only `env` within `labels` | `"labels": {"env": "prod"}` |
| `attributes.http.method` | Keep only `http.method` within `attributes` | `"attributes": {"http.method": "GET"}` |

**Conflict resolution**: if both `labels` and `labels.env` are specified,
the broader selector wins — keep entire `labels`.

### 1.4 Behavior per Format

- **JSON**: fields not in the list are removed from each record object.
- **Table**: only listed fields appear as columns.
- **CSV**: only listed fields appear as columns.

### 1.5 Absent Fields

If a specified field does not exist in a record, it is silently ignored
(no error, no null placeholder). Different records may have different
field sets.

### 1.6 Examples

```bash
# Metric: skip points array (biggest token saver)
obz metric query -q 'up' --fields name,labels,stats.avg

# Log: only core fields
obz log search -q 'error' --fields timestamp,service,message

# Trace: overview without attributes
obz trace get abc123 --fields span_id,parent_span_id,service,name,duration_us,status
```

---

## 2. `--truncate` — Value Truncation

### 2.1 Syntax

```
--truncate <max_chars>
```

A single integer: the maximum character length for any string value in
the output. Applies to all string values recursively.

### 2.2 Scope

`--truncate` operates on **all string values** in the serialized output,
including:

- Top-level fields (e.g., `message`, `name`)
- Nested fields (e.g., `attributes.http.request_body`)
- Array elements (e.g., `events[].attributes.exception.stacktrace`)
- Extension data values

It does **not** affect:

- Field **names** (keys)
- Non-string values (numbers, booleans, null)
- The envelope (`status`, `metadata` fields)

### 2.3 Truncation Format

When a string value exceeds `max_chars`:

```
"<first max_chars characters>...[truncated, <original_length> chars]"
```

Example with `--truncate 80`:

```json
{
  "message": "java.lang.NullPointerException: Cannot invoke method on null\n\tat com.example...[truncated, 48231 chars]"
}
```

The marker tells the agent:
1. This value was truncated.
2. The original length (so the agent can decide whether to fetch the
   full value via a different query).

When any values are truncated, the response `metadata` includes a count:

```json
{
  "metadata": {
    "provider": "dev-vl",
    "total_count": 10,
    "truncated_values": 3
  }
}
```

This lets agents detect truncation without scanning individual fields.
The field is omitted when no truncation occurs.

### 2.4 Character Boundary Safety

Truncation respects UTF-8 character boundaries. If `max_chars` falls in
the middle of a multi-byte character, truncate to the previous character
boundary.

### 2.5 Behavior per Format

- **JSON**: string values truncated in the JSON output.
- **Table**: string values truncated (in addition to existing table cell
  truncation at 60 chars, `--truncate` takes priority if smaller).
- **CSV**: string values truncated in CSV cells.

### 2.6 Examples

```bash
# Prevent any single value from exceeding 500 chars
obz log search -q 'error' --truncate 500

# Combine with --fields for maximum control
obz trace get abc123 --fields service,name,attributes --truncate 200
```

---

## 3. Combination Behavior

When both flags are specified:

1. **`--fields` runs first** — remove unwanted fields.
2. **`--truncate` runs second** — cap remaining string values.

This order is more efficient (fewer values to scan after field
projection) and more intuitive (truncate what you chose to keep).

---

## 4. Interaction with `--view` (future)

When `--view` is implemented (v0.2.0+):

- `--view agent` applies its own field filtering first (e.g., strips
  `extensions`, `resource`), then `--fields` further narrows the result.
- `--view full` + `--fields` works as expected — full data, then
  project.
- `--truncate` always applies last, regardless of view mode.

---

## 5. Implementation Notes

### 5.1 Where in the Pipeline

```
Provider.query()
  → Result<T>
  → serde_json::to_value()          // serialize
  → project_fields(value, fields)   // --fields (new)
  → truncate_values(value, max)     // --truncate (new)
  → print_json / print_table / print_csv
```

Both transformations happen in `output.rs`, keeping providers and execute
functions unaware.

### 5.2 `format_and_print` Signature Change

```rust
pub fn format_and_print<T: Serialize>(
    data: &T,
    format: OutputFormat,
    fields: Option<&[String]>,   // NEW
    truncate: Option<usize>,     // NEW
    writer: &mut impl Write,
) -> io::Result<()>
```

All existing callers in `execute.rs` pass `None, None` initially.
The obz shell (`dispatch.rs`) passes the parsed CLI values.

### 5.3 Field Projection Algorithm

```
project_fields(value, fields):
    data = value["data"]           // dig into envelope
    for each field in data:
        if field value is Array<Object>:
            for each object in array:
                keep only keys matching fields list
                for dot-notation fields like "labels.env":
                    keep only "env" within "labels"
        else:
            skip (preserves result_type, trace summary, string arrays, etc.)
```

The algorithm automatically discovers object arrays inside the `data`
section. It does not hard-code array key names (`series`, `entries`,
`spans`), so new response types are supported without code changes.
String arrays (e.g., `services` in `trace_detail`) are not projected.

### 5.4 Value Truncation Algorithm

```
--truncate <CHARS>   Truncate string values longer than CHARS characters (min: 1)
```

### 5.5 CLI Registration

Both flags go into the shared query args (`with_query_global_args()` in
`cli.rs`), making them available to all query commands.

```
--fields <FIELDS>    Comma-separated list of fields to include (dot notation supported)
--truncate <CHARS>   Truncate string values longer than CHARS characters
```

---

## 6. Applicable Commands

| Command | `--fields` targets | `--truncate` targets |
|---------|-------------------|---------------------|
| `metric query` | series[] | all string values in series |
| `metric list` | items[] (strings, limited use) | items (limited use) |
| `metric info` | — (single object, not projected) | info field values |
| `metric labels` | items[] (strings, limited use) | items (limited use) |
| `metric label-values` | items[] (strings, limited use) | items (limited use) |
| `metric series` | series[] label maps | label values |
| `log search` | entries[] | all string values in entries |
| `trace search` | spans[] | all string values in spans |
| `trace get` | spans[] (not summary) | all string values in spans |
| Extension commands | data[] elements | all string values |

> For string list commands (metric list/labels/label-values), `--fields`
> has limited applicability since items are plain strings, not objects.
> `--truncate` works normally.
