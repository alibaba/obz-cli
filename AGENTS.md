# obz

Multi-backend observability CLI (Rust). Query metrics, logs, and traces across
12 backends with a unified interface, optimized for AI Agents.

## Language

- Code, comments, doc comments, commit messages: **English only**.

## Commands

```bash
# IMPORTANT: use ~/.cargo/bin/ (Rust 1.94+), not /usr/local/bin/rustc (1.67).
export PATH="$HOME/.cargo/bin:$PATH"

# Build
cargo build

# Format check
cargo fmt --check

# Lint (treat warnings as errors)
cargo clippy --workspace --all-targets -- -D warnings

# Test
cargo test --workspace

# License / dependency audit
cargo deny check licenses bans sources

# Full verify (run before every commit)
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

## Definition of Done

A task is complete when ALL pass:

1. `cargo fmt --check` exits 0
2. `cargo clippy --workspace --all-targets -- -D warnings` exits 0
3. `cargo test --workspace` exits 0
4. `cargo deny check licenses bans sources` exits 0

## Commit Format

[Conventional Commits](https://www.conventionalcommits.org/):
`<type>(<scope>): <description>`

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`

## Escalation Rules

- If tests fail after 3 attempts: stop, report the failing test with full output.
- If a dependency conflict arises: check `Cargo.toml` and `deny.toml` first, then ask.
- **Never**: suppress warnings with `#[allow(...)]` without justification, delete
  failing tests, use `todo!()` / `unimplemented!()` / `dbg!()` (blocked by pre-commit hook).

## Prohibitions

- `unsafe` code (`unsafe_code = "forbid"` in workspace lints)
- `openssl` / `openssl-sys` (banned in `deny.toml`; use `rustls` + `ring`)
- Type suppression: no `as` casts to silence type errors
- Empty `match` arms or `catch-all` patterns that swallow errors
- Adding dependencies without justification (prefer >1M downloads, Apache-2.0/MIT)

---

## Architecture

Three-layer design inspired by DuckDB's extension system.

```
obz (shell)              CLI parsing (clap), config, dispatch. NO business logic.
  -> obz-core (framework)  Data models, provider traits, registry, execution, output.
  -> obz-providers (ext)   Backend adapters. Implements core traits. NO clap dependency.
```

### Dependency Direction

```
obz -> obz-core, obz-providers
obz-providers -> obz-core
obz-core -> (no business dependencies)
```

**obz-core never depends on obz-providers.** Providers are injected via traits.

### Layer Boundaries

| Belongs in | obz-core | obz-providers | obz |
|---|:-:|:-:|:-:|
| clap types / derives | | | Y |
| HTTP client (reqwest) | | Y | |
| Backend API parsing | | Y | |
| Data models / traits | Y | | |
| Trait implementations | | Y | |
| Command execution logic | Y | | |
| Provider instantiation | | | Y |
| Output formatting | Y | | |

### Workspace Layout

```
crates/
  core/        obz-core
  providers/   obz-providers (vm, vl, vt, dd, sls, prom, jg, os, es, mimir, loki, tempo)
  obz/         obz binary (main.rs, cli.rs, dispatch.rs, help.rs, config.rs, skills.rs, credential_process.rs, credential_cache.rs, manpage.rs, resolve.rs)
```

## When Writing Code

### Error Handling

- Use `thiserror` for all error types. Primary type: `ObzError` in `crates/core/src/model/error.rs`.
- Propagate with `?`. Map external errors with `map_err(|e| ObzError::Network { ... })`.
- When mapping errors, populate `recoverable` (can the caller retry?),
  `source_chain` (underlying library errors), and `suggestion` (actionable fix) where applicable.
- Required config: `config.require("endpoint")?` returns `ObzError::InvalidArgument`.
- Never use `unwrap()` / `expect()` in library code (core/providers).

### Async

- Traits use `#[async_trait]` for object safety (`dyn MetricProvider`).
- Single-threaded tokio runtime created in `main.rs` only: `Builder::new_current_thread()`.
- Providers are `Send + Sync`.
- Execute functions (e.g. `execute_metric_query`) accept `output: OutputFormat`
  and `writer: &mut impl Write` — formatting and output are handled in one step.

### HTTP

- All providers use shared helpers in `crates/providers/src/util.rs`:
  `build_http_client()`, `send_request()`, `send_and_parse_json()`.
- Auth: `apply_standard_auth()` — bearer token > basic auth.
- Verbose output: `[verbose] -> ...` / `[verbose] <- ...` via `eprintln!`.

### Serialization

- `serde` derives everywhere. Common attrs: `rename_all`, `skip_serializing_if = "Option::is_none"`, `default`.
- Response types go in `response.rs`. Conversion to core models goes in `convert.rs`.
- JSON key order preserved via `serde_json` `preserve_order` feature.

### Visibility

- Default to `pub(crate)` in providers. Only `register_all()` is `pub`.
- Core re-exports key types at crate root via `pub use` in `lib.rs`.

### Module Structure per Provider

```
crates/providers/src/<name>/
  mod.rs       Registration (meta(), build()), re-exports sub-modules
  response.rs  Serde types for the backend's JSON API
  convert.rs   response -> core model conversion
```

Shared modules (`promql/`, `jaegerapi/`, `util.rs`) are reusable across providers —
they are **not** standalone providers (no `meta()`, not in `register_all`).
If adding a PromQL-compatible backend, reuse `promql::PromqlMetricProvider`.

### Import Order

`std` -> third-party -> workspace crates -> local modules. Separate groups with blank lines.

### Doc Comments

- `///` for items, `//!` for module-level. Include design intent, not just "what".
- Workspace lint: `doc_markdown = "warn"`, `missing_errors_doc = "warn"`, `missing_panics_doc = "warn"`.

## When Adding a New Provider

1. Create `crates/providers/src/<name>/` with `mod.rs`, `response.rs`, `convert.rs`
2. Implement `MetricProvider` / `LogProvider` / `TraceProvider` (+ `ExtensionProvider` if needed) with `#[async_trait]`
3. Add `pub(crate) fn meta() -> ProviderMeta` in `mod.rs`
4. Add `registry.register(<name>::meta())` in `crates/providers/src/lib.rs`
5. **`crates/obz/src/main.rs` does not change.**

## When Reviewing / Debugging

- Run `cargo clippy --workspace --all-targets -- -D warnings` after every change.
- Check `cargo deny check` if dependencies were modified.
- Extension flags declared `required: false` at clap level; enforcement is at runtime after provider resolution.

## Testing

- Unit tests: `#[cfg(test)] mod tests` in the same file.
- Assertions: `assert_eq!` / `assert!`. No external mock framework currently.
- Config tests use `tempfile` for temp directories.
- Run a single test: `cargo test -p obz-core -- test_name`

## Config Resolution

Priority: CLI flags (`--endpoint`, `--timeout`) > credential-process output > `config.yaml` (with `${env:}`/`${file:}` resolved).
CLI flags always win so users can override credential-process output with one-off flags.
Config dir: `~/.config/obz/` or `$OBZ_CONFIG_DIR`.
Auth credentials are configured exclusively in `config.yaml` under `providers.<name>.auth`.
No CLI credential flags (`--token`, `--username`, `--password`) or environment variable
overrides (`OBZ_<TYPE>_<KEY>`) exist.

### Variable References

Config values support variable references resolved at load time:
- `${env:VAR}` — environment variable, **error** if unset (safe default)
- `${env?:VAR}` — environment variable, **empty string** if unset (explicit opt-in)
- `${file:path}` — file contents; `~` expansion; relative to config dir;
  trims one trailing newline; empty file = error; non-UTF-8 = error
  **Path policy**: absolute paths and `../` traversal are allowed by design —
  the user controls `config.yaml`, so restricting paths would break legitimate
  use cases like `${file:~/.obz/sls-ak.txt}` or `/etc/obz/shared-token`
- No nesting: `${env:${file:x}}` is an error

### credential-process

External command to dynamically obtain credentials at runtime.
Configured under `providers.<name>.auth.credential-process`.

```yaml
providers:
  es-prod:
    provider: es
    endpoint: https://es.example.com:9200
    auth:
      username: elastic
      credential-process:
        command: vault
        args: ["kv", "get", "-format=json", "secret/es-prod"]
        timeout: 10s
```

**Execution**: `Command::new(command).args(args)` — no shell, no quoting issues.

**JSON protocol** (stdout):
```json
{
  "version": 1,
  "expiration": "2026-04-17T12:00:00Z",
  "token": "...", "username": "...", "password": "...",
  "access-key-id": "...", "access-key-secret": "...",
  "api-key": "...", "app-key": "...",
  "headers": { "x-custom": "value" }
}
```

- `version` must be `1`; other values → error
- `expiration` is used for credential cache expiration
- Non-empty auth fields override inline config values
- `headers` merge with config headers (credential-process wins on conflict);
  reserved and provider-managed headers still trigger hard errors
- Empty-string fields are treated as absent

**Timing**: executed only for query commands (`metric`, `log`, `trace`);
never for `provider list`, `provider check`, `completions`, or `skills` commands.

**Timeout**: default 30s, configurable via `timeout` field.

**Error mapping**:
- Command not found → `InvalidArgument(ConfigError)`
- Non-zero exit → `Auth(AuthMissing)` with stderr (truncated to 512 bytes)
- Timeout → `Network(Timeout)`, child process killed
- Invalid JSON / wrong version → `InvalidArgument(ConfigError)`

### Credential Caching

When `credential-process` is configured, obz caches the output on disk to
avoid re-executing the external command on every invocation.

- **Cache directory**: `$XDG_CACHE_HOME/obz/credential-cache/` or `~/.cache/obz/credential-cache/`
- **Cache file**: `<provider-name>.json` (permissions `0600`)
- **Expiration**: protocol `expiration` field > config `cache-ttl` > never expires
- **Buffer**: cache is considered expired 60 seconds before `expires_at`
- **Concurrent access**: per-provider `.lock` file with exclusive flock
- **Atomic writes**: tempfile + rename to prevent corruption
- **401 handling**: cache is invalidated and refreshed; the current request
  returns an error with `suggestion: "Credentials have been refreshed. Retry the same command."`

Example:
```yaml
providers:
  vm:
    endpoint: http://localhost:8428
    auth:
      token: ${env:OBZ_VM_TOKEN}
  sls:
    auth:
      access-key-id: ${file:~/.obz/sls-ak.txt}
      access-key-secret: ${file:~/.obz/sls-sk.txt}
```

`Debug` output auto-redacts keys containing `token`, `password`, `secret` as substrings,
or keys equal to `key` / ending with `-key` (e.g. `api-key`). Keys like `monkey` are not redacted.
