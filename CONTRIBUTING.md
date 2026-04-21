# Contributing to obz

Thank you for your interest in contributing!

## Before You Start

- **Bug fixes and typos** — go straight to a PR.
- **New features or design changes** — please [open an issue](https://github.com/alibaba/obz-cli/issues/new) first to discuss.

## Development Setup

1. Install [Rust](https://rustup.rs/) (MSRV: **1.75**)
2. Clone and build:

```bash
git clone https://github.com/alibaba/obz-cli.git
cd obz-cli
cargo build
```

3. Install the pre-commit hook:

```bash
./scripts/install-hooks.sh
```

## Verification

Run the full check before submitting a PR — this is what CI runs:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check licenses bans sources
```

## Code Style

- `cargo fmt` for formatting.
- `cargo clippy` with `-D warnings`.
- No `unsafe` code (`unsafe_code = "forbid"`).
- No `todo!()`, `unimplemented!()`, or `dbg!()`.
- Default to `pub(crate)` visibility in provider crates.
- See [AGENTS.md](AGENTS.md) for architecture details.

## Commit Messages

[Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>
```

Types: `feat`, `fix`, `refactor`, `test`, `docs`, `chore`

## Testing

- Unit tests live in `#[cfg(test)] mod tests` in the same file.
- Run a single test: `cargo test -p obz-core -- test_name`
- Bug fixes should include a regression test when feasible.
- New providers should include response fixture tests in `convert.rs`.

## Pull Request Process

1. One logical change per PR.
2. All CI checks must pass (see [Verification](#verification)).
3. A maintainer will review your PR. Please be patient — we aim to
   respond within a few business days.
4. If your PR changes user-facing behavior, update relevant documentation.

## Adding a New Provider

See [AGENTS.md](AGENTS.md) for the full guide. Short version:

1. Create `crates/providers/src/<name>/` with `mod.rs`, `response.rs`, `convert.rs`
2. Implement provider traits (`MetricProvider` / `LogProvider` / `TraceProvider`)
3. Register in `crates/providers/src/lib.rs`
4. Add a skill document in `skills/obz-<name>/SKILL.md`

## License

By contributing, you agree that your contributions will be licensed under the
[Apache License 2.0](LICENSE).
