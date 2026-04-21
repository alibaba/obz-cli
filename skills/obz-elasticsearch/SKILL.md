---
name: obz-elasticsearch
description: >
  Elasticsearch provider for obz. Supports log search and trace operations using
  Elasticsearch Query DSL (Lucene-based). Requires --index flag. This skill
  should be used when the user mentions "Elasticsearch", "obz -p es", or needs
  to search logs or traces from an Elasticsearch cluster.
---

# obz-elasticsearch: Elasticsearch Provider

## Quick Reference

| Field            | Value                                              |
|------------------|----------------------------------------------------|
| Aliases          | `es`, `elasticsearch`                              |
| Signals          | Log, Trace                                         |
| Query language   | Elasticsearch Query DSL (Lucene-based)             |
| Auth             | Basic auth, bearer token, or API key (in config.yaml) |
| Provider flags   | `--index` (required at runtime)                    |
| Supported cmds   | log search, trace search, trace get                |

## Supported Commands

```
obz log search       # Search log entries in an Elasticsearch index
obz trace search     # Search trace spans in an Elasticsearch index
obz trace get        # Retrieve a trace by ID from Elasticsearch
```

## Provider-Specific Flags

| Flag        | Type   | Required | Description                              |
|-------------|--------|----------|------------------------------------------|
| `--index`   | String | Yes      | Index name or pattern to query           |

The `--index` flag is declared optional at the clap level (so other
providers don't error), but Elasticsearch enforces it at runtime.

## Authentication

Configure auth in `config.yaml` under `providers.<name>.auth`.
Priority: API key > bearer token > basic auth.

**Basic auth**:
```yaml
# config.yaml
providers:
  es:
    endpoint: https://es.example.com
    index: logs-*
    auth:
      username: elastic
      password: ${env:ES_PASS}
```

**Bearer token**:
```yaml
    auth:
      token: ${env:ES_TOKEN}
```

**API key** (`Authorization: ApiKey <key>`):
```yaml
    auth:
      api-key: ${env:ES_API_KEY}
```

Then query with just `-p`:
```bash
obz log search -p es -q 'level:ERROR' --from now-1h
```

## Query Language

Elasticsearch accepts Lucene-based query strings in the `-q` parameter.
The syntax is nearly identical to OpenSearch.

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

- Field names are case-sensitive and depend on your index mapping.
- Use double quotes for exact phrase matches.
- Leading wildcards (`*error`) are disabled by default for performance.

## Elasticsearch vs OpenSearch

Both providers share the same query syntax and flag surface. The main
difference is the backend API version and endpoint format.

| Aspect            | Elasticsearch                | OpenSearch              |
|-------------------|------------------------------|-------------------------|
| Alias             | `es`                         | `os`                    |
| Auth options      | Basic + bearer + API key     | Basic + bearer          |
| API lineage       | Elastic N.V.                 | AWS fork                |

If your cluster is OpenSearch, use the `os` provider instead.

## Examples

**Search logs by level**:
```bash
obz log search -p es --index 'logs-*' -q 'level:ERROR' --from now-1h
```

**Search with boolean operators**:
```bash
obz log search -p es --index 'app-logs' \
  -q 'service:api AND status:500' --from now-6h
```

**Search traces**:
```bash
obz trace search -p es --index 'traces-*' -q 'service:frontend' --from now-1h
```

**Retrieve a specific trace**:
```bash
obz trace get -p es --index 'traces-*' abc123def456
```
