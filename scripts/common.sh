#!/usr/bin/env bash
# ══════════════════════════════════════════════════════════
# obz CLI — shared functions and constants
# ══════════════════════════════════════════════════════════
#
# Sourced by other scripts — do not execute directly.

[ -n "$_COMMON_SH_LOADED" ] && return 0
_COMMON_SH_LOADED=1

set -euo pipefail

# ── Colors ───────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
BOLD='\033[1m'
NC='\033[0m'

info()  { printf "${BLUE}[info]${NC}  %s\n" "$*"; }
ok()    { printf "${GREEN}[ ok ]${NC}  %s\n" "$*"; }
warn()  { printf "${YELLOW}[warn]${NC}  %s\n" "$*"; }
err()   { printf "${RED}[error]${NC} %s\n" "$*"; exit 1; }
step()  { printf "\n${BOLD}══ %s ══${NC}\n\n" "$*"; }

# ── Paths ────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
DIST_DIR="$PROJECT_DIR/dist"
VERSION_FILE="$PROJECT_DIR/VERSION"

# ── Build constants ──────────────────────────────────────

BIN_NAME="obz"
LINUX_TARGETS=("x86_64-unknown-linux-gnu" "aarch64-unknown-linux-gnu")
MAC_TARGETS=("aarch64-apple-darwin" "x86_64-apple-darwin")
WINDOWS_TARGETS=("x86_64-pc-windows-gnu")
ALL_TARGETS=("${LINUX_TARGETS[@]}" "${MAC_TARGETS[@]}" "${WINDOWS_TARGETS[@]}")
ALL_PLATFORMS=("linux-x86_64" "linux-aarch64" "darwin-x86_64" "darwin-aarch64" "windows-x86_64")

NATIVE_TARGET="$(rustc -vV 2>/dev/null | grep host | awk '{print $2}' || echo '')"
HOST_OS="$(uname -s)"

# Cross-platform SHA256 (macOS has shasum, not sha256sum)
sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        err "Neither sha256sum nor shasum is installed"
    fi
}

# ── Version management ───────────────────────────────────

# Read base version from VERSION file (no channel suffix).
read_base_version() {
    [ -f "$VERSION_FILE" ] || err "VERSION file not found: $VERSION_FILE"
    local v
    v="$(cat "$VERSION_FILE" | tr -d '[:space:]')"
    [ -n "$v" ] || err "VERSION file is empty"
    echo "$v"
}

# Write version to VERSION + Cargo.toml (workspace).
write_version() {
    local version="$1"
    echo "$version" > "$VERSION_FILE"
    # sed -i.bak is compatible with both GNU sed and BSD sed (macOS)
    sed -i.bak "s/^version = \".*\"/version = \"${version}\"/" "$PROJECT_DIR/Cargo.toml"
    rm -f "$PROJECT_DIR/Cargo.toml.bak"
    grep -q "^version = \"${version}\"" "$PROJECT_DIR/Cargo.toml" \
        || err "Failed to sync version to Cargo.toml"
}

# Target triple → platform label
target_to_platform() {
    local target="$1"
    case "$target" in
        x86_64-unknown-linux-gnu)   echo "linux-x86_64" ;;
        aarch64-unknown-linux-gnu)  echo "linux-aarch64" ;;
        x86_64-apple-darwin)        echo "darwin-x86_64" ;;
        aarch64-apple-darwin)       echo "darwin-aarch64" ;;
        x86_64-pc-windows-gnu)      echo "windows-x86_64" ;;
        *) echo "unknown" ;;
    esac
}
