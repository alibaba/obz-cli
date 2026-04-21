---
name: obz-opensearch
description: >
  OpenSearch provider for obz. Supports log search and trace operations using
  Lucene-based query syntax. Requires --index flag. This skill should be used
  when the user mentions "OpenSearch", "obz -p os", or needs to search logs or
  traces from an OpenSearch cluster.
---

# obz-opensearch: OpenSearch Provider

## Quick Reference

| Field            | Value                                              |
|------------------|----------------------------------------------------|
| Aliases          | `os`, `opensearch`                                 |
| Signals          | Log, Trace                                         |
| Query language   | OpenSearch DSL (Lucene-based query string)         |
| Auth             | Basic auth or bearer token (in config.yaml)        |
| Provider flags   | `--index` (required at runtime)                    |
| Supported cmds   | log search, trace search, trace get                |

## Supported Commands

```
obz log search       # Search log entries in an OpenSearch index
obz trace search     # Search trace spans in an OpenSearch index
obz trace get        # Retrieve a trace by ID from OpenSearch
```

## Provider-Specific Flags

| Flag        | Type   | Required | Description                              |
|-------------|--------|----------|------------------------------------------|
| `--index`   | String | Yes      | Index name or pattern to query           |

The `--index` flag is declared optional at the clap level (so other
providers don't error), but OpenSearch enforces it at runtime. Omitting
it will produce a clear error message.

## Authentication

Configure auth in `config.yaml` under `providers.<name>.auth`. Supports
either basic auth or bearer token.

**Basic auth**:
```yaml
# config.yaml
providers:
  os:
    endpoint: https://opensearch.example.com
    index: logs-*
    auth:
      username: admin
      password: ${env:OS_PASS}
```

**Bearer token**:
```yaml
providers:
  os:
    endpoint: https://opensearch.example.com
    index: logs-*
    auth:
      token: ${env:OS_TOKEN}
```

Then query with just `-p`:
```bash
obz log search -p os -q 'level:ERROR' --from now-1h
```

## Query Language

OpenSearch accepts Lucene-based query strings in the `-q` parameter.

### Common Patterns

```
level:ERROR                          # field match
level:ERROR AND service:api          # boolean AND
status:(404 OR 500)                  # grouped OR
message:"connection refused"         # phrase match
host:web-* AND NOT path:/health      # wildcards and negation
response_time:[500 TO *]             # range query
```

### Tips

- Field names are case-sensitive and must match your index mapping.
- Use double quotes for exact phrase matches.
- Wildcards (`*`, `?`) work in field values but not at the start of a term
  by default (it's expensive).

## Examples

**Search logs by level**:
```bash
obz log search -p os --index 'logs-*' -q 'level:ERROR' --from now-1h
```

**Search with boolean operators**:
```bash
obz log search -p os --index 'app-logs' \
  -q 'service:api AND status:500' --from now-6h
```

**Search traces**:
```bash
obz trace search -p os --index 'traces-*' -q 'service:frontend' --from now-1h
```

**Retrieve a specific trace**:
```bash
obz trace get -p os --index 'traces-*' abc123def456
```
