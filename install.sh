#!/bin/sh
# obz CLI — install script
#
# Usage:
#   curl -sSL https://raw.githubusercontent.com/alibaba/obz-cli/main/install.sh | sh
#   curl -sSL .../install.sh | OBZ_VERSION=0.2.0 sh        # pin version
#   curl -sSL .../install.sh | OBZ_INSTALL_DIR=~/bin sh     # custom dir
#
# Environment variables:
#   OBZ_INSTALL_DIR   Install directory (default: /usr/local/bin or ~/.local/bin)
#   OBZ_VERSION       Pin a specific version (default: latest)
#   OBZ_BASE_URL      Override download base URL

set -e

# ── Colors ───────────────────────────────────────────────

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()  { printf "${BLUE}[info]${NC}  %s\n" "$1"; }
ok()    { printf "${GREEN}[ ok ]${NC}  %s\n" "$1"; }
warn()  { printf "${YELLOW}[warn]${NC}  %s\n" "$1"; }
error() { printf "${RED}[error]${NC} %s\n" "$1"; exit 1; }

# ── Configuration ────────────────────────────────────────

# TODO: Update to GitHub Releases URL once the repository is public
# GITHUB_REPO="alibaba/obz-cli"
# OBZ_BASE_URL="${OBZ_BASE_URL:-https://github.com/${GITHUB_REPO}/releases/download}"
# OBZ_LATEST_URL="https://api.github.com/repos/${GITHUB_REPO}/releases/latest"
OBZ_BASE_URL="${OBZ_BASE_URL:-}"
OBZ_LATEST_URL=""
OBZ_INSTALL_DIR="${OBZ_INSTALL_DIR:-}"
BINARY_NAME="obz"

# ── Detect platform ─────────────────────────────────────

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux)   OS="linux" ;;
        Darwin)  OS="darwin" ;;
        MINGW*|MSYS*|CYGWIN*)
            OS="windows"
            BINARY_NAME="obz.exe"
            warn "Windows support is experimental — consider using WSL"
            ;;
        *)
            error "Unsupported OS: $OS (supported: Linux, macOS, Windows/MSYS)" ;;
    esac

    case "$ARCH" in
        x86_64|amd64)    ARCH="x86_64" ;;
        aarch64|arm64)   ARCH="aarch64" ;;
        armv7l)
            error "32-bit ARM is not supported — please use a 64-bit system" ;;
        i686|i386)
            error "32-bit x86 is not supported — please use a 64-bit system" ;;
        *)
            error "Unsupported CPU architecture: $ARCH (supported: x86_64, aarch64)" ;;
    esac

    PLATFORM="${OS}-${ARCH}"
    info "Detected platform: ${OS} ${ARCH}"
}

# ── Detect downloader ───────────────────────────────────

detect_downloader() {
    if command -v curl > /dev/null 2>&1; then
        DOWNLOADER="curl"
    elif command -v wget > /dev/null 2>&1; then
        DOWNLOADER="wget"
    else
        error "curl or wget is required — please install one first"
    fi
}

download() {
    url="$1"
    output="$2"
    info "Downloading $url"
    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL --connect-timeout 10 --retry 3 --retry-delay 2 -o "$output" "$url"
    else
        wget -q --timeout=10 --tries=3 -O "$output" "$url"
    fi
}

download_text() {
    url="$1"
    if [ "$DOWNLOADER" = "curl" ]; then
        curl -fsSL --connect-timeout 10 --retry 3 "$url" 2>/dev/null
    else
        wget -q --timeout=10 --tries=3 -O- "$url" 2>/dev/null
    fi
}

# ── Determine install directory ─────────────────────────

determine_install_dir() {
    if [ -n "$OBZ_INSTALL_DIR" ]; then
        mkdir -p "$OBZ_INSTALL_DIR" 2>/dev/null || true
        INSTALL_DIR="$OBZ_INSTALL_DIR"
    elif [ -w /usr/local/bin ]; then
        INSTALL_DIR="/usr/local/bin"
    elif [ -d "$HOME/.local/bin" ] || mkdir -p "$HOME/.local/bin" 2>/dev/null; then
        INSTALL_DIR="$HOME/.local/bin"
        check_path_contains "$INSTALL_DIR"
    else
        mkdir -p "$HOME/bin" 2>/dev/null || true
        INSTALL_DIR="$HOME/bin"
        warn "Installing to $INSTALL_DIR"
        check_path_contains "$INSTALL_DIR"
    fi

    info "Install directory: $INSTALL_DIR"
}

check_path_contains() {
    dir="$1"
    case ":$PATH:" in
        *":$dir:"*) ;;
        *)
            warn "$dir is not in PATH!"
            warn "Add it to your shell config:"
            if [ -f "$HOME/.zshrc" ]; then
                warn "  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.zshrc && source ~/.zshrc"
            elif [ -f "$HOME/.bashrc" ]; then
                warn "  echo 'export PATH=\"$dir:\$PATH\"' >> ~/.bashrc && source ~/.bashrc"
            else
                warn "  export PATH=\"$dir:\$PATH\""
            fi
            ;;
    esac
}

# ── Check existing installation ─────────────────────────

check_existing() {
    if command -v obz > /dev/null 2>&1; then
        EXISTING_VERSION="$(obz --version 2>/dev/null | awk '{print $2}' || echo "unknown")"
        EXISTING_PATH="$(command -v obz)"
        info "Found existing installation: $EXISTING_VERSION ($EXISTING_PATH)"

        if [ "$EXISTING_VERSION" = "$OBZ_VERSION" ]; then
            warn "Version v${OBZ_VERSION} is already installed"

            if [ -e /dev/tty ]; then
                printf "  ${YELLOW}Overwrite? [y/N]${NC} "
                read -r answer < /dev/tty 2>/dev/null || answer="n"
                case "$answer" in
                    [yY]|[yY][eE][sS])
                        info "Overwriting v${OBZ_VERSION}..."
                        ;;
                    *)
                        ok "Skipped"
                        exit 0
                        ;;
                esac
            else
                info "No interactive terminal — overwriting by default"
            fi
        else
            info "Updating $EXISTING_VERSION -> $OBZ_VERSION"
        fi
    fi
}

# ── Verify SHA256 checksum ──────────────────────────────

verify_checksum() {
    file="$1"
    expected="$2"

    if [ -z "$expected" ]; then
        warn "Skipping checksum verification (no sha256 available)"
        return 0
    fi

    actual=""
    if command -v sha256sum > /dev/null 2>&1; then
        actual="$(sha256sum "$file" | awk '{print $1}')"
    elif command -v shasum > /dev/null 2>&1; then
        actual="$(shasum -a 256 "$file" | awk '{print $1}')"
    else
        warn "Skipping checksum verification (sha256sum/shasum not available)"
        return 0
    fi

    if [ "$actual" != "$expected" ]; then
        error "SHA256 checksum mismatch!\n  Expected: $expected\n  Actual:   $actual\nFile may be corrupted or tampered with — please retry"
    fi
    ok "SHA256 checksum verified"
}

# ── Main ────────────────────────────────────────────────

main() {
    printf "\n"
    printf "${BLUE}  obz CLI Installer${NC}\n"
    printf "  A multi-backend observability CLI tool\n\n"

    detect_platform
    detect_downloader

    # Resolve version
    if [ -n "${OBZ_VERSION:-}" ]; then
        info "Using specified version: v${OBZ_VERSION}"
    else
        info "Checking latest version..."
        OBZ_VERSION="$(download_text "$OBZ_LATEST_URL" | tr -d '[:space:]')" || true
        if [ -z "$OBZ_VERSION" ]; then
            OBZ_VERSION="0.1.0"
            warn "Could not fetch latest version — using default ${OBZ_VERSION}"
        fi
        ok "Latest version: v${OBZ_VERSION}"
    fi

    determine_install_dir
    check_existing

    # Construct download URL
    TARBALL="obz-${OBZ_VERSION}-${PLATFORM}.tar.gz"
    DOWNLOAD_URL="${OBZ_BASE_URL}/${TARBALL}"
    CHECKSUM_URL="${DOWNLOAD_URL}.sha256"

    # Temp directory with cleanup
    TMP_DIR="$(mktemp -d 2>/dev/null || mktemp -d -t 'obz-install')"
    trap 'rm -rf "$TMP_DIR"' EXIT

    # Download
    info "Downloading obz v${OBZ_VERSION} (${PLATFORM})..."
    if ! download "$DOWNLOAD_URL" "$TMP_DIR/$TARBALL"; then
        echo ""
        error "Download failed! Possible causes:
  1. Version v${OBZ_VERSION} does not exist
  2. Platform ${PLATFORM} is not available
  3. Network connectivity issue

  Supported platforms:
    - linux-x86_64     (Linux x86 64-bit)
    - linux-aarch64    (Linux ARM 64-bit)
    - darwin-x86_64    (macOS Intel)
    - darwin-aarch64   (macOS Apple Silicon)
    - windows-x86_64   (Windows 64-bit)

  Alternative: build from source
    git clone <repo> && cd obz-cli && cargo install --path crates/obz"
    fi

    # Verify checksum
    EXPECTED_SHA="$(download_text "$CHECKSUM_URL" | awk '{print $1}' || echo "")"
    verify_checksum "$TMP_DIR/$TARBALL" "$EXPECTED_SHA"

    # Extract
    info "Extracting..."
    tar xzf "$TMP_DIR/$TARBALL" -C "$TMP_DIR"

    if [ ! -f "$TMP_DIR/$BINARY_NAME" ]; then
        error "Binary not found after extraction — archive may be corrupted"
    fi

    # Install
    chmod +x "$TMP_DIR/$BINARY_NAME"

    if [ -w "$INSTALL_DIR" ]; then
        mv "$TMP_DIR/$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
    else
        warn "$INSTALL_DIR is not writable — trying ~/.local/bin"
        mkdir -p "$HOME/.local/bin" 2>/dev/null || true
        if [ -w "$HOME/.local/bin" ]; then
            INSTALL_DIR="$HOME/.local/bin"
            mv "$TMP_DIR/$BINARY_NAME" "$INSTALL_DIR/$BINARY_NAME"
            check_path_contains "$INSTALL_DIR"
        else
            error "Cannot write to any install directory. Try: OBZ_INSTALL_DIR=~/bin sh install.sh"
        fi
    fi

    # Verify installation & detect path conflicts
    if ! command -v obz > /dev/null 2>&1; then
        warn "obz was installed to $INSTALL_DIR/$BINARY_NAME but is not in PATH"
        export PATH="$INSTALL_DIR:$PATH"
        check_path_contains "$INSTALL_DIR"
    else
        RESOLVED_PATH="$(command -v obz)"
        INSTALLED_VERSION="$("$INSTALL_DIR/$BINARY_NAME" --version 2>/dev/null | awk '{print $2}')"

        # Resolve symlinks for conflict detection
        RESOLVED_REAL="$(readlink -f "$RESOLVED_PATH" 2>/dev/null || echo "$RESOLVED_PATH")"
        INSTALLED_REAL="$(readlink -f "$INSTALL_DIR/$BINARY_NAME" 2>/dev/null || echo "$INSTALL_DIR/$BINARY_NAME")"

        if [ "$RESOLVED_REAL" != "$INSTALLED_REAL" ]; then
            EXISTING_VERSION="$("$RESOLVED_PATH" --version 2>/dev/null | awk '{print $2}' || echo "unknown")"
            RESOLVED_DIR="$(dirname "$RESOLVED_PATH")"
            warn "Path conflict detected!"
            warn "  Installed to: $INSTALL_DIR/$BINARY_NAME (v${OBZ_VERSION})"
            warn "  PATH resolves: $RESOLVED_PATH (v${EXISTING_VERSION})"
            warn ""
            warn "A higher-priority obz in PATH shadows this installation."
            warn "Options:"
            warn ""
            if [ -w "$RESOLVED_DIR" ]; then
                warn "  1. Remove the old binary:  rm \"$RESOLVED_PATH\""
            else
                warn "  1. Remove the old binary:  sudo rm \"$RESOLVED_PATH\""
            fi
            warn "  2. Reinstall to that dir:  OBZ_INSTALL_DIR=$RESOLVED_DIR sh install.sh"
        else
            ok "Installed successfully! obz v${INSTALLED_VERSION}"
        fi
    fi

    printf "\n"
    printf "${GREEN}  Installation complete!${NC}\n\n"
    printf "  Quick start:\n"
    printf "    obz --help                     Show help\n"
    printf "    obz metric query -p vm \\\\       Query metrics\n"
    printf "      --endpoint http://... \\\\\n"
    printf "      -q 'up' --from now-1h\n"
    printf "    obz log search -p vl \\\\         Search logs\n"
    printf "      --endpoint http://... \\\\\n"
    printf "      -q '*' --from now-15m\n"
    printf "\n"
}

main "$@"
