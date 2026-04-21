#!/usr/bin/env bash
# ══════════════════════════════════════════════════════════
# obz CLI — cross-platform build script
# ══════════════════════════════════════════════════════════
#
# Compiles release binaries for all platforms, verifies artifacts, and
# packages them into tar.gz archives with SHA256 checksums.
#
# Usage:
#   ./scripts/build.sh                     # all 5 platforms
#   ./scripts/build.sh --linux-only        # Linux x86_64 + aarch64
#   ./scripts/build.sh --mac-only          # macOS x86_64 + aarch64
#   ./scripts/build.sh --windows-only      # Windows x86_64
#   ./scripts/build.sh --version 0.2.0     # override version
#
# Prerequisites:
#   - cargo, rustup
#   - zig >= 0.13 + cargo-zigbuild  (for cross-compilation)
#   - macOS SDK (SDKROOT)            (for Linux → macOS cross-compilation, optional)

source "$(cd "$(dirname "$0")" && pwd)/common.sh"

# ── Arguments ────────────────────────────────────────────

DO_LINUX=true
DO_MAC=true
DO_WINDOWS=true
INPUT_VERSION=""

while [ $# -gt 0 ]; do
    case "$1" in
        --linux-only)   DO_MAC=false; DO_WINDOWS=false; shift ;;
        --mac-only)     DO_LINUX=false; DO_WINDOWS=false; shift ;;
        --windows-only) DO_LINUX=false; DO_MAC=false; shift ;;
        --version)
            [ $# -ge 2 ] || err "--version requires an argument (e.g. 0.2.0)"
            INPUT_VERSION="$2"
            echo "$INPUT_VERSION" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+$' \
                || err "Invalid version: $INPUT_VERSION (expected X.Y.Z)"
            shift 2 ;;
        --help|-h)
            cat <<'HELP'
Usage: ./scripts/build.sh [OPTIONS]

Options:
  --version X.Y.Z   Set version (writes to VERSION + Cargo.toml)
  --linux-only      Build only Linux (x86_64 + aarch64)
  --mac-only        Build only macOS (x86_64 + aarch64)
  --windows-only    Build only Windows (x86_64)
  --help, -h        Show this help

Without --version, uses the version from the VERSION file.
HELP
            exit 0 ;;
        *) err "Unknown argument: $1 (use --help)" ;;
    esac
done

# ── Version ──────────────────────────────────────────────

if [ -n "$INPUT_VERSION" ]; then
    write_version "$INPUT_VERSION"
    ok "Version set to ${INPUT_VERSION}"
fi

VERSION="$(read_base_version)"

echo ""
printf "${BOLD}  obz CLI Build — v${VERSION}${NC}\n"
echo ""

# ── Prerequisites ────────────────────────────────────────

check_prerequisites() {
    step "Prerequisites"

    command -v cargo >/dev/null 2>&1 || err "cargo is not installed"
    ok "cargo $(cargo --version 2>/dev/null | awk '{print $2}')"

    local need_zigbuild=false
    if [ "$HOST_OS" = "Darwin" ]; then
        $DO_LINUX && need_zigbuild=true
        $DO_WINDOWS && need_zigbuild=true
    else
        need_zigbuild=true
    fi

    if $need_zigbuild; then
        command -v zig >/dev/null 2>&1 || err "zig is not installed (need >= 0.13)"
        ok "zig $(zig version 2>/dev/null)"

        command -v cargo-zigbuild >/dev/null 2>&1 || err "cargo-zigbuild is not installed (cargo install cargo-zigbuild)"
        ok "cargo-zigbuild installed"
    else
        info "Native build only, zig/cargo-zigbuild not required"
    fi

    if $DO_MAC && [ "$HOST_OS" != "Darwin" ]; then
        if [ -n "${SDKROOT:-}" ] && [ -d "$SDKROOT" ]; then
            ok "macOS SDK found: $SDKROOT"
        else
            warn "macOS SDK not found. Set SDKROOT to your macOS SDK path for cross-compilation."
            warn "On macOS: export SDKROOT=\$(xcrun --sdk macosx --show-sdk-path)"
            warn "Skipping macOS targets."
            DO_MAC=false
        fi
    fi

    # Ensure Rust targets are installed
    local targets_to_check=()
    $DO_LINUX && targets_to_check+=("${LINUX_TARGETS[@]}")
    $DO_MAC && targets_to_check+=("${MAC_TARGETS[@]}")
    $DO_WINDOWS && targets_to_check+=("${WINDOWS_TARGETS[@]}")

    local installed
    installed="$(rustup target list --installed)"
    for target in "${targets_to_check[@]}"; do
        if ! echo "$installed" | grep -q "$target"; then
            info "Installing Rust target: $target"
            rustup target add "$target"
        fi
    done
    ok "Rust targets ready"
}

# ── Build ────────────────────────────────────────────────

build_targets() {
    local label="$1"
    shift
    local targets=("$@")

    step "Build ${label}"

    cd "$PROJECT_DIR"
    mkdir -p "$DIST_DIR"

    for target in "${targets[@]}"; do
        local platform
        platform="$(target_to_platform "$target")"

        info "Building ${platform} (${target})..."

        if [ "$HOST_OS" = "Darwin" ] && [[ "$target" == *apple-darwin* ]]; then
            info "  (native cargo build)"
            cargo build --release --target "$target" 2>&1 | tail -3
        elif [[ "$target" == *apple-darwin* ]]; then
            info "  (cross: cargo zigbuild + SDKROOT)"
            SDKROOT="${SDKROOT:?SDKROOT must be set for macOS cross-compilation}" \
                cargo zigbuild --release --target "$target" 2>&1 | tail -3
        else
            info "  (cargo zigbuild)"
            cargo zigbuild --release --target "$target" 2>&1 | tail -3
        fi

        # Windows binaries need .exe extension
        local bin_filename="$BIN_NAME"
        [[ "$target" == *windows* ]] && bin_filename="${BIN_NAME}.exe"

        local binary="$PROJECT_DIR/target/$target/release/${bin_filename}"
        [ -f "$binary" ] || err "${target} build failed: $binary not found"

        local tarball="${BIN_NAME}-${VERSION}-${platform}.tar.gz"
        tar czf "$DIST_DIR/$tarball" -C "$(dirname "$binary")" "$bin_filename"
        sha256_of "$DIST_DIR/$tarball" > "$DIST_DIR/${tarball}.sha256"

        local size
        size="$(du -h "$DIST_DIR/$tarball" | awk '{print $1}')"
        ok "${platform}: ${size} -> dist/${tarball}"
    done
}

# ── Verify ───────────────────────────────────────────────

VERIFY_TOTAL=0
VERIFY_PASSED=0
VERIFY_FAILED=0

verify_err() { printf "${RED}[FAIL]${NC}  %s\n" "$1"; }

verify_target() {
    local target="$1"
    local platform
    platform="$(target_to_platform "$target")"
    local os="${platform%%-*}"
    local arch="${platform##*-}"

    local bin_filename="$BIN_NAME"
    [[ "$target" == *windows* ]] && bin_filename="${BIN_NAME}.exe"

    local binary="$PROJECT_DIR/target/$target/release/$bin_filename"
    local tarball="$DIST_DIR/${BIN_NAME}-${VERSION}-${platform}.tar.gz"

    VERIFY_TOTAL=$((VERIFY_TOTAL + 1))

    if [ ! -f "$binary" ]; then
        verify_err "[${platform}] Binary not found: $binary"
        VERIFY_FAILED=$((VERIFY_FAILED + 1))
        return 1
    fi

    local file_info
    file_info="$(file "$binary")"

    # Check binary format
    local expect_format
    case "$os" in
        linux)   expect_format="ELF" ;;
        darwin)  expect_format="Mach-O" ;;
        windows) expect_format="PE32\|PE32+\|MS-DOS\|Windows" ;;
    esac
    if ! echo "$file_info" | grep -qi "$expect_format"; then
        verify_err "[${platform}] Wrong format: expected $expect_format, got: $file_info"
        VERIFY_FAILED=$((VERIFY_FAILED + 1))
        return 1
    fi

    # Check architecture
    local expect_arch
    case "$arch" in
        x86_64)  expect_arch="x86.64\|x86_64\|X86.64" ;;
        aarch64) expect_arch="aarch64\|ARM64\|arm64" ;;
    esac
    if ! echo "$file_info" | grep -qi "$expect_arch"; then
        verify_err "[${platform}] Wrong arch: expected $arch, got: $file_info"
        VERIFY_FAILED=$((VERIFY_FAILED + 1))
        return 1
    fi

    # Size sanity check (1MB - 50MB)
    local size_bytes
    size_bytes="$(stat -c%s "$binary" 2>/dev/null || stat -f%z "$binary" 2>/dev/null)"
    if [ "$size_bytes" -lt 1048576 ] || [ "$size_bytes" -gt 52428800 ]; then
        verify_err "[${platform}] Suspicious file size: ${size_bytes} bytes"
        VERIFY_FAILED=$((VERIFY_FAILED + 1))
        return 1
    fi

    # Run on native target
    if [ "$target" = "$NATIVE_TARGET" ]; then
        local version_output
        if version_output="$("$binary" --version 2>&1)"; then
            ok "[${platform}] Run check: $version_output"
        else
            verify_err "[${platform}] Failed to run"
            VERIFY_FAILED=$((VERIFY_FAILED + 1))
            return 1
        fi
    fi

    # Verify tarball
    if [ -f "$tarball" ]; then
        if ! tar tzf "$tarball" | grep -q "$BIN_NAME"; then
            verify_err "[${platform}] $BIN_NAME not found in tarball"
            VERIFY_FAILED=$((VERIFY_FAILED + 1))
            return 1
        fi

        local sha_file="${tarball}.sha256"
        if [ -f "$sha_file" ]; then
            local expected actual
            expected="$(awk '{print $1}' "$sha_file")"
            actual="$(sha256_of "$tarball")"
            if [ "$expected" != "$actual" ]; then
                verify_err "[${platform}] SHA256 mismatch"
                VERIFY_FAILED=$((VERIFY_FAILED + 1))
                return 1
            fi
        fi
        ok "[${platform}] Tarball + SHA256 verified"
    fi

    local size_h
    size_h="$(du -h "$binary" | awk '{print $1}')"
    ok "[${platform}] PASS (${expect_format} ${arch}, ${size_h})"
    VERIFY_PASSED=$((VERIFY_PASSED + 1))
}

verify_all() {
    step "Verify artifacts"

    local targets=()
    $DO_LINUX && targets+=("${LINUX_TARGETS[@]}")
    $DO_MAC && targets+=("${MAC_TARGETS[@]}")
    $DO_WINDOWS && targets+=("${WINDOWS_TARGETS[@]}")

    for target in "${targets[@]}"; do
        verify_target "$target" || true
    done

    echo ""
    if [ "$VERIFY_FAILED" -eq 0 ]; then
        ok "Verification: ${VERIFY_PASSED}/${VERIFY_TOTAL} passed"
    else
        printf "${RED}[FAIL]${NC}  Verification: ${VERIFY_PASSED}/${VERIFY_TOTAL} passed, ${VERIFY_FAILED} failed\n"
        exit 1
    fi
}

# ── Main ─────────────────────────────────────────────────

check_prerequisites

if $DO_LINUX && $DO_MAC && $DO_WINDOWS; then
    build_targets "all platforms (Linux + macOS + Windows)" "${ALL_TARGETS[@]}"
elif $DO_LINUX; then
    build_targets "Linux (x86_64 + aarch64)" "${LINUX_TARGETS[@]}"
elif $DO_MAC; then
    build_targets "macOS (x86_64 + aarch64)" "${MAC_TARGETS[@]}"
elif $DO_WINDOWS; then
    build_targets "Windows (x86_64)" "${WINDOWS_TARGETS[@]}"
fi

verify_all

# List artifacts
step "Artifacts"
_found=false
for _p in "${ALL_PLATFORMS[@]}"; do
    _f="$DIST_DIR/${BIN_NAME}-${VERSION}-${_p}.tar.gz"
    [ -f "$_f" ] && ls -lh "$_f" && _found=true
done
$_found || warn "No artifacts found"

ok "Build complete: v${VERSION}"
