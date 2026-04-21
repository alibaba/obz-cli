# Installation

## Quick Install (recommended)

```bash
curl -sSL https://raw.githubusercontent.com/alibaba/obz-cli/main/install.sh | sh
```

The install script automatically detects your OS and architecture, downloads the
latest release binary, verifies the SHA256 checksum, and installs it to your PATH.

Options:

```bash
# Pin a specific version
curl -sSL https://raw.githubusercontent.com/alibaba/obz-cli/main/install.sh | OBZ_VERSION=0.1.0 sh

# Custom install directory
curl -sSL https://raw.githubusercontent.com/alibaba/obz-cli/main/install.sh | OBZ_INSTALL_DIR=~/bin sh
```

## From Source

Requires [Rust](https://rustup.rs/) 1.75 or later.

```bash
git clone https://github.com/alibaba/obz-cli.git
cd obz-cli
cargo install --path crates/obz
```

## From GitHub Releases

Download the binary for your platform from [Releases](https://github.com/alibaba/obz-cli/releases).

| Platform | Archive |
|---|---|
| Linux x86_64 | `obz-<version>-linux-x86_64.tar.gz` |
| Linux aarch64 | `obz-<version>-linux-aarch64.tar.gz` |
| macOS Intel | `obz-<version>-darwin-x86_64.tar.gz` |
| macOS Apple Silicon | `obz-<version>-darwin-aarch64.tar.gz` |
| Windows x86_64 | `obz-<version>-windows-x86_64.zip` |

Extract and move the binary to a directory in your PATH:

```bash
tar xzf obz-<version>-<platform>.tar.gz
mv obz /usr/local/bin/
```

## Verify Installation

```bash
obz --version
```
