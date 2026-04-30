# obz

[English](README.md)

[![CI](https://github.com/alibaba/obz-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/alibaba/obz-cli/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/alibaba/obz-cli?sort=semver&display_name=tag)](https://github.com/alibaba/obz-cli/releases)
[![Downloads](https://img.shields.io/github/downloads/alibaba/obz-cli/total)](https://github.com/alibaba/obz-cli/releases)
[![License](https://img.shields.io/github/license/alibaba/obz-cli)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.75%2B-orange.svg)](Cargo.toml)

多后端可观测性 CLI，统一查询 metrics、logs、traces — AI Agent 友好。当前支持 10+ 种后端，语义化查询能力将在后续版本中支持。

## 为什么用 obz？

**问题**：可观测数据分散在多种后端中——Prometheus、Loki、Jaeger、Elasticsearch、Datadog 等。每个后端有各自的 CLI、查询语言和输出格式。没有统一的方式跨后端查询，尤其对需要结构化、可预期响应的 AI Agent 来说更是如此。

**obz 当前做了什么**：一个 CLI 查询 10+ 种后端的 metrics、logs、traces。结构化 JSON 输出、确定的退出码、内置技能文档——让 AI Agent 可以可靠调用，不需要处理各后端的输出差异。

**obz 的方向**：兼容并扩展 OTel Semantic Conventions，将语义化能力引入查询侧。Agent 可以先发现有哪些信号、理解信号的含义，再决定怎么查询——而不是盲目拼查询语句。

## 特性

- **多后端统一接口** — 一套命令查询 10+ 种后端：VictoriaMetrics、Prometheus、Grafana Mimir、VictoriaLogs、Grafana Loki、VictoriaTraces、Jaeger、Grafana Tempo、OpenSearch、Elasticsearch、阿里云 SLS、Datadog
- **Agent-first** — 默认 JSON 输出，结构化错误响应（错误分类、退出码、是否可重试、修复建议），内置各 Provider 的 AI Agent 技能文档，支持输出投影（--fields、--truncate）减少 token 消耗
- **后端透传** — 使用后端原生查询语言（MetricsQL、PromQL、LogsQL、LogQL、TraceQL、DQL 等），不发明 DSL
- **可扩展** — 三层架构（CLI 层 / 核心框架 / Provider 适配层），依赖方向单向；新增后端只需实现 Provider trait 并注册，不需要改动核心框架
- **配置文件** — 在 `~/.config/obz/` 中预配置 endpoint 和凭据，查询时只需 `-p <name>`

## 安装

```bash
curl -sSL https://raw.githubusercontent.com/alibaba/obz-cli/main/install.sh | sh
```

其他安装方式请参阅[安装指南](docs/installation.md)。

## 快速开始

```bash
# 查询 VictoriaMetrics 指标
obz metric query -p vm --endpoint http://localhost:8428 -q 'up'

# 查询 Prometheus 指标
obz metric query -p prom --endpoint http://localhost:9090 -q 'up'

# 查询 Grafana Mimir 指标（多租户通过配置文件 headers 设置）
obz metric query -p mimir --endpoint http://localhost:9009 -q 'up'

# 搜索 VictoriaLogs 日志
obz log search -p vl --endpoint http://localhost:9428 -q 'error' --from now-1h

# 搜索 Grafana Loki 日志
obz log search -p loki --endpoint http://localhost:3100 -q '{job="varlogs"}' --from now-1h

# 搜索 VictoriaTraces 链路追踪
obz trace search -p vt --endpoint http://localhost:10428 -q 'frontend'

# 搜索 Jaeger 链路追踪
obz trace search -p jg --endpoint http://localhost:16686 -q 'frontend'

# 搜索 Grafana Tempo 链路追踪
obz trace search -p tempo --endpoint http://localhost:3200 --from now-1h

# 搜索 OpenSearch 日志
obz log search -p os --endpoint http://localhost:9200 --index 'otel-logs-*' -q 'error' --from now-1h

# 搜索 Elasticsearch 日志
obz log search -p es --endpoint http://localhost:9200 --index 'logs-*' -q 'error' --from now-1h

# 查询阿里云 SLS 指标（PromQL 兼容，凭据来自 config.yaml）
obz metric query -p sls --project my-proj --metricstore prom-store -q 'up'

# 搜索 Datadog 日志（凭据来自 config.yaml）
obz log search -p dd -q 'service:web status:error' --from now-1h

# 或者配置一次，之后只需 -p 即可查询
obz metric query -p vm -q 'up'              # 使用 ~/.config/obz/config.yaml
obz log search -p sls -q 'error'            # 凭据来自 config.yaml
```

## 命令列表

```
obz metric query          执行指标查询（即时 / 范围）
obz metric list           列举指标名
obz metric info           获取指标元数据
obz metric labels         列举标签名
obz metric label-values   列举指定标签的值
obz metric series         查找匹配的时间线
obz log search            搜索日志条目
obz trace search          搜索 span
obz trace get             按 Trace ID 获取完整 trace
obz trace services        列举可用的服务名（VT / Jaeger）
obz trace operations      列举服务的操作名（VT / Jaeger）
obz trace tags            列举可用的 tag 名称（Tempo）
obz trace tag-values      列举指定 tag 的值（Tempo）
obz provider list         列出已注册的 Provider
obz provider check        检查 Provider 配置是否有效
obz completions           生成 shell 自动补全脚本
obz skills                输出面向 AI Agent 的 Provider 技能文档
```

各 Provider 的详细用法请参见 [skills](skills/) 目录 — 同时面向人类参考和 AI Agent 消费。

## Provider 支持

### Metric

| 命令 | VM | Prom | Mimir | SLS | DD |
|------|:--:|:----:|:-----:|:---:|:--:|
| `metric query` | MetricsQL | PromQL | PromQL | PromQL | Datadog Query |
| `metric list` | 支持 | 支持 | 支持 | 支持 | 支持 |
| `metric info` | 支持 | 支持 | 支持 | — | 支持 |
| `metric labels` | 支持 | 支持 | 支持 | 支持 | — |
| `metric label-values` | 支持 | 支持 | 支持 | 支持 | — |
| `metric series` | 支持 | 支持 | 支持 | 支持 | — |

### Log

| 命令 | VL | Loki | OS | ES | SLS | DD |
|------|:--:|:----:|:--:|:--:|:---:|:--:|
| `log search` | LogsQL | LogQL | OpenSearch DSL | ES Query DSL | SLS Query | DD Log Query |

### Trace

| 命令 | VT | Jaeger | Tempo | OS | ES | SLS | DD |
|------|:--:|:------:|:-----:|:--:|:--:|:---:|:--:|
| `trace search` | 支持 | 支持 | 支持 | 支持 | 支持 | 支持 | 支持 |
| `trace get` | 支持 | 支持 | 支持 | 支持 | 支持 | 支持 | 支持 |
| `trace services` | 支持 | 支持 | — | — | — | — | — |
| `trace operations` | 支持 | 支持 | — | — | — | — | — |
| `trace tags` | — | — | 支持 | — | — | — | — |
| `trace tag-values` | — | — | 支持 | — | — | — | — |

**Provider 别名**：`vm`（VictoriaMetrics）、`vl`（VictoriaLogs）、`vt`（VictoriaTraces）、`sls`（阿里云 SLS）、`dd`（Datadog）、`prom`（Prometheus）、`jg`（Jaeger）、`os`（OpenSearch）、`es`（Elasticsearch）、`mimir`（Grafana Mimir）、`loki`（Grafana Loki）、`tempo`（Grafana Tempo）

## 配置文件

在 `~/.config/obz/config.yaml`（或 `OBZ_CONFIG_DIR` 指定的目录）中预配置 Provider。认证信息统一放在各 Provider 的 `auth:` 下：

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

  es-prod:
    provider: es
    endpoint: https://es.example.com:9200
    auth:
      credential-process:
        command: vault
        args: ["kv", "get", "-format=json", "secret/es-prod"]
        timeout: 10s
```

支持的认证字段：

- `token` — Bearer Token 认证
- `username` / `password` — Basic Auth 认证
- `access-key-id` / `access-key-secret` — 阿里云 SLS
- `api-key` / `app-key` — Datadog
- `credential-process` — 运行时动态获取凭据

配置值支持变量引用，在加载时解析：

- `${env:VAR}` — 环境变量（未设置则**报错**）
- `${env?:VAR}` — 环境变量（未设置时返回空字符串）
- `${file:path}` — 文件内容（相对路径以配置目录为基准，支持 `~` 展开）

配置完成后，只需 `-p <name>` 即可查询：

```bash
obz metric query -p vm -q 'up'
obz metric query -p mimir -q 'up'
obz log search -p loki -q '{job="varlogs"}' --from now-1h
obz log search -p sls -q 'error' --from now-1h
```

优先级：CLI flags > credential-process > config.yaml（含 `${env:}` / `${file:}` 解析后的值）。

完整配置示例请参见 [examples/config/](examples/config/)。

## 文档

- [CLI 命令接口规范](docs/spec/cli-interface.md)
- [Provider Skills](skills/) — 各 Provider 的用法、查询语言指南和示例
- [Metric 数据模型](docs/spec/metric-model.md)
- [Log 数据模型](docs/spec/log-model.md)
- [Trace 数据模型](docs/spec/trace-model.md)

## 路线图

- **Semantic Conventions 查询** — 兼容并扩展 OTel Semantic Conventions，支持从标准 YAML 仓库或独立服务进行语义化数据发现和查询
- **持续扩展后端覆盖** — 根据社区需求新增后端
- **文档站** — 部署独立文档站，提供使用指南、示例和 API 参考

## 许可证

本项目采用 [Apache License 2.0](LICENSE) 许可。
