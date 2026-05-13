# obz

[中文](README.zh-CN.md)

[![CI](https://github.com/alibaba/obz-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/alibaba/obz-cli/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/alibaba/obz-cli?sort=semver&display_name=tag)](https://github.com/alibaba/obz-cli/releases)
[![Downloads](https://img.shields.io/github/downloads/alibaba/obz-cli/total)](https://github.com/alibaba/obz-cli/releases)
[![License](https://img.shields.io/github/license/alibaba/obz-cli)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](Cargo.toml)

A multi-backend observability CLI for metrics, logs, and traces — unified interface, AI-Agent friendly. Currently supports 10+ backends, with semantic querying planned for future releases.

## Why obz?

**The problem**: Observability data is scattered across multiple backends — Prometheus, Loki, Jaeger, Elasticsearch, Datadog, and more. Each has its own CLI, query language, and output format. There is no unified way to query across them, especially for AI Agents that need structured, predictable responses.

**What obz does today**: One CLI to query metrics, logs, and traces across 10+ backends. Structured JSON output, deterministic exit codes, built-in skill documents — designed so AI Agents can call it reliably without parsing surprises.

**Where obz is heading**: Integrating and extending OTel Semantic Conventions to bring semantic awareness to the query side. Agents will be able to discover what signals exist, understand their meaning, and know how to query them — before writing a single query.

## Features

- **Unified interface** — One set of commands for 10+ backends: VictoriaMetrics, Prometheus, Grafana Mimir, GreptimeDB, VictoriaLogs, Grafana Loki, VictoriaTraces, Jaeger, Grafana Tempo, OpenSearch, Elasticsearch, Alibaba Cloud SLS, and Datadog
- **Agent-first** — Default JSON output with structured error responses (category, exit code, recoverability, fix suggestions), built-in per-provider skill documents for AI Agents, and output projection (`--fields`, `--truncate`) to reduce token usage
- **Backend passthrough** — Uses native query languages (MetricsQL, PromQL, LogsQL, LogQL, TraceQL, DQL, etc.)
- **Extensible** — Three-layer architecture (CLI / core framework / provider adapters) with one-way dependencies; adding a new backend means implementing a provider trait and registering it — no changes to the core
- **Config file** — Pre-configure endpoints and credentials in `~/.config/obz/`, then query with just `-p <name>`

## Installation

```bash
curl -sSL https://raw.githubusercontent.com/alibaba/obz-cli/main/install.sh | sh
```

For other installation methods, see the [Installation Guide](docs/installation.md).

## Quick Start

```bash
# Query metrics from VictoriaMetrics
obz metric query -p vm --endpoint http://localhost:8428 -q 'up'

# Query metrics from Prometheus
obz metric query -p prom --endpoint http://localhost:9090 -q 'up'

# Query metrics from Grafana Mimir (multi-tenant via config headers)
obz metric query -p mimir --endpoint http://localhost:9009 -q 'up'

# Search logs from VictoriaLogs
obz log search -p vl --endpoint http://localhost:9428 -q 'error' --from now-1h

# Search logs from Grafana Loki
obz log search -p loki --endpoint http://localhost:3100 -q '{job="varlogs"}' --from now-1h

# Search traces from VictoriaTraces
obz trace search -p vt --endpoint http://localhost:10428 -q 'frontend'

# Search traces from Jaeger
obz trace search -p jg --endpoint http://localhost:16686 -q 'frontend'

# Search traces from Grafana Tempo
obz trace search -p tempo --endpoint http://localhost:3200 --from now-1h

# Search logs from OpenSearch
obz log search -p os --endpoint http://localhost:9200 --index 'otel-logs-*' -q 'error' --from now-1h

# Search logs from Elasticsearch
obz log search -p es --endpoint http://localhost:9200 --index 'logs-*' -q 'error' --from now-1h

# Query metrics from Alibaba Cloud SLS (PromQL compatible, credentials from config.yaml)
obz metric query -p sls --project my-proj --metricstore prom-store -q 'up'

# Search logs from Datadog (credentials from config.yaml)
obz log search -p dd -q 'service:web status:error' --from now-1h

# Or configure once, then query with just -p
obz metric query -p vm -q 'up'              # uses ~/.config/obz/config.yaml
obz log search -p sls -q 'error'            # credentials from config.yaml
```

## Commands

```
obz metric query          Execute a metric query (instant or range)
obz metric list           List metric names
obz metric info           Get metric metadata
obz metric labels         List label names
obz metric label-values   List values for a specific label
obz metric series         Find series matching selectors
obz log search            Search for log entries
obz trace search          Search for spans across traces
obz trace get             Get all spans for a specific trace by ID
obz trace services        List available service names (VT/Jaeger)
obz trace operations      List operations for a service (VT/Jaeger)
obz trace tags            List available tag names (Tempo)
obz trace tag-values      List values for a specific tag (Tempo)
obz provider list         List built-in providers
obz provider check        Validate provider configuration
obz completions           Generate shell completion scripts
obz skills                Show provider skill documents
```

Per-provider usage details are available in the [skills](skills/) directory — designed for both human reference and AI agent consumption.

## Provider Support

### Metric

| Command | VM | Prom | Mimir | Greptime | SLS | DD |
|---------|:--:|:----:|:-----:|:--------:|:---:|:--:|
| `metric query` | MetricsQL | PromQL | PromQL | PromQL | PromQL | Datadog Query |
| `metric list` | Yes | Yes | Yes | — | Yes | Yes |
| `metric info` | Yes | Yes | Yes | — | — | Yes |
| `metric labels` | Yes | Yes | Yes | Yes | Yes | — |
| `metric label-values` | Yes | Yes | Yes | Yes | Yes | — |
| `metric series` | Yes | Yes | Yes | Yes | Yes | — |

### Log

| Command | VL | Loki | OS | ES | SLS | DD |
|---------|:--:|:----:|:--:|:--:|:---:|:--:|
| `log search` | LogsQL | LogQL | OpenSearch DSL | ES Query DSL | SLS Query | DD Log Query |

### Trace

| Command | VT | Jaeger | Tempo | OS | ES | SLS | DD |
|---------|:--:|:------:|:-----:|:--:|:--:|:---:|:--:|
| `trace search` | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| `trace get` | Yes | Yes | Yes | Yes | Yes | Yes | Yes |
| `trace services` | Yes | Yes | — | — | — | — | — |
| `trace operations` | Yes | Yes | — | — | — | — | — |
| `trace tags` | — | — | Yes | — | — | — | — |
| `trace tag-values` | — | — | Yes | — | — | — | — |

**Provider aliases**: `vm` (VictoriaMetrics), `vl` (VictoriaLogs), `vt` (VictoriaTraces), `sls` (Alibaba Cloud SLS), `dd` (Datadog), `prom` (Prometheus), `greptime` (GreptimeDB), `jg` (Jaeger), `os` (OpenSearch), `es` (Elasticsearch), `mimir` (Grafana Mimir), `loki` (Grafana Loki), `tempo` (Grafana Tempo)

## Configuration

Pre-configure providers in `~/.config/obz/config.yaml` (or in the directory pointed to by
`OBZ_CONFIG_DIR`). Authentication is configured per provider under `auth:`:

```yaml
# config.yaml
providers:
  vm:
    provider: vm
    endpoint: http://localhost:8428
    auth:
      token: ${env:OBZ_VM_TOKEN}

  mimir:
    provider: mimir
    endpoint: http://localhost:9009
    headers:
      X-Scope-OrgID: my-tenant
    auth:
      username: ${env:MIMIR_USERNAME}
      password: ${file:~/.secrets/mimir-password.txt}

  sls:
    provider: sls
    endpoint: https://my-proj.cn-hangzhou.log.aliyuncs.com
    project: my-proj
    metricstore: prom-store
    logstore: nginx
    auth:
      access-key-id: ${file:~/.obz/sls-ak.txt}
      access-key-secret: ${file:~/.obz/sls-sk.txt}

  dd:
    provider: dd
    endpoint: https://api.datadoghq.com
    auth:
      api-key: ${env:DD_API_KEY}
      app-key: ${env:DD_APP_KEY}

  greptime:
    provider: greptimedb
    endpoint: http://localhost:4000  # root URL only; no path, query, or fragment
    db: public                      # required; GreptimeDB database name

  es-prod:
    provider: es
    endpoint: https://es.example.com:9200
    auth:
      credential-process:
        command: vault
        args: ["kv", "get", "-format=json", "secret/es-prod"]
        timeout: 10s
```

Supported auth fields include:

- `token` for bearer auth
- `username` and `password` for basic auth
- `access-key-id` and `access-key-secret` for Alibaba Cloud SLS
- `api-key` and `app-key` for Datadog
- `credential-process` for dynamic credential fetching at query time

Variable references are resolved when loading config:

- `${env:VAR}` — environment variable (**error** if unset)
- `${env?:VAR}` — environment variable (empty string if unset)
- `${file:path}` — file contents (path relative to config directory, `~` expanded)

Then query with just `-p <name>`:

```bash
obz metric query -p vm -q 'up'
obz metric query -p mimir -q 'up'
obz log search -p loki -q '{job="varlogs"}' --from now-1h
obz log search -p sls -q 'error' --from now-1h
```

Priority: CLI flags > credential-process > config.yaml (including `${env:}` / `${file:}` resolved values). Use `OBZ_CONFIG_DIR` to customize the config directory.

See [examples/config/](examples/config/) for full configuration examples.

## Documentation

- [CLI Interface Spec](docs/spec/cli-interface.md) — Design specification
- [Provider Skills](skills/) — Per-provider usage, query language guides, and examples
- [Metric Data Model](docs/spec/metric-model.md)
- [Log Data Model](docs/spec/log-model.md)
- [Trace Data Model](docs/spec/trace-model.md)

## Roadmap

- **Semantic Conventions querying** — Integrate and extend OTel Semantic Conventions to support semantic-aware data discovery and querying from standard YAML repositories or standalone schema services
- **Expand provider coverage** — Add new backends based on community demand
- **Documentation site** — Deploy a dedicated documentation site with guides, examples, and API reference

## License

Licensed under the [Apache License, Version 2.0](LICENSE).
