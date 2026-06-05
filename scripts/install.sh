#!/bin/sh
#
# install.sh – Install the Eye compiler from GitHub Releases.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/<owner>/eye/main/scripts/install.sh | sh
#
# Environment variables:
#   EYE_VERSION     - Release tag (default: latest)
#   EYE_PREFIX      - Install prefix  (default: /usr/local)
#   EYE_TARGET      - Override auto-detected target triple
#
# Detects the platform and architecture, downloads the matching release
# archive from GitHub, and extracts the `eye` (and `eye-lsp`) binaries.

set -eu

# ── Configuration ──────────────────────────────────────────────────────
REPO="${EYE_REPO:-anomalyco/eye}"
VERSION="${EYE_VERSION:-latest}"
PREFIX="${EYE_PREFIX:-/usr/local}"
TARGET="${EYE_TARGET:-}"

# ── Help ───────────────────────────────────────────────────────────────
usage() {
    cat <<EOF
Usage: curl -fsSL https://raw.githubusercontent.com/$REPO/main/scripts/install.sh | sh

Installs the Eye compiler to $PREFIX/bin/eye (and eye-lsp).

Environment variables:
  EYE_VERSION  Release tag (default: latest)
  EYE_PREFIX   Install prefix (default: /usr/local)
  EYE_TARGET   Override target triple auto-detection
EOF
    exit 0
}

[ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ] && usage

# ── Platform detection ─────────────────────────────────────────────────
detect_target() {
    local arch
    local os

    arch="$(uname -m)"
    os="$(uname -s)"

    case "$os" in
        Linux)  os="unknown-linux-gnu"    ;;
        Darwin) os="apple-darwin"         ;;
        MINGW*|MSYS*|CYGWIN*)
            os="pc-windows-msvc"
            echo "error: Windows is not supported by this script; download the .zip from GitHub Releases." >&2
            exit 1
            ;;
        *)
            echo "error: unsupported OS: $os" >&2
            exit 1
            ;;
    esac

    case "$arch" in
        x86_64|amd64) arch="x86_64"       ;;
        aarch64|arm64) arch="aarch64"     ;;
        *)
            echo "error: unsupported architecture: $arch" >&2
            exit 1
            ;;
    esac

    echo "${arch}-${os}"
}

if [ -z "$TARGET" ]; then
    TARGET="$(detect_target)"
fi

# ── Resolve version ────────────────────────────────────────────────────
if [ "$VERSION" = "latest" ]; then
    echo "Fetching latest release tag..." >&2
    VERSION="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
        | grep '"tag_name":' \
        | sed 's/.*"tag_name": "\(.*\)",.*/\1/')"
    if [ -z "$VERSION" ]; then
        echo "error: could not determine latest release" >&2
        exit 1
    fi
    echo "Latest release: $VERSION" >&2
fi

# ── Download ───────────────────────────────────────────────────────────
ARCHIVE="eye-${TARGET}.tar.gz"
URL="https://github.com/$REPO/releases/download/$VERSION/$ARCHIVE"
TMPDIR="$(mktemp -d 2>/dev/null || mktemp -d -t eye-install)"

cleanup() { rm -rf "$TMPDIR"; }
trap cleanup EXIT

echo "Downloading $URL ..." >&2
curl -fsSL "$URL" -o "$TMPDIR/$ARCHIVE"

# ── Verify checksums (if available) ────────────────────────────────────
CHECKSUMS_URL="https://github.com/$REPO/releases/download/$VERSION/checksums.txt"
if CHECKSUMS="$(curl -fsSL "$CHECKSUMS_URL" 2>/dev/null)"; then
    echo "Verifying checksum..." >&2
    cd "$TMPDIR"
    echo "$CHECKSUMS" | grep "$ARCHIVE" > checksums.txt || true
    if [ -s checksums.txt ]; then
        if command -v sha256sum >/dev/null 2>&1; then
            sha256sum -c checksums.txt >/dev/null 2>&1 || {
                echo "error: checksum verification failed" >&2
                exit 1
            }
        elif command -v shasum >/dev/null 2>&1; then
            shasum -a 256 -c checksums.txt >/dev/null 2>&1 || {
                echo "error: checksum verification failed" >&2
                exit 1
            }
        fi
        echo "Checksum OK" >&2
    fi
    cd - >/dev/null
fi

# ── Extract ────────────────────────────────────────────────────────────
echo "Extracting..." >&2
tar xzf "$TMPDIR/$ARCHIVE" -C "$TMPDIR"
EXTRACTED="$TMPDIR/eye-${TARGET}"

# ── Install ────────────────────────────────────────────────────────────
BINDIR="$PREFIX/bin"
mkdir -p "$BINDIR"

install_binary() {
    local src="$1"
    local name="$2"
    if [ -f "$src" ]; then
        cp "$src" "$BINDIR/$name"
        chmod 755 "$BINDIR/$name"
        echo "Installed $BINDIR/$name" >&2
    fi
}

install_binary "$EXTRACTED/eye" "eye"
install_binary "$EXTRACTED/eye-lsp" "eye-lsp"

# ── Check PATH ─────────────────────────────────────────────────────────
case ":$PATH:" in
    *:"$BINDIR":*) ;;
    *)
        echo "warning: $BINDIR is not in PATH. Add it:" >&2
        echo "  export PATH=\"\$PATH:$BINDIR\"" >&2
        ;;
esac

echo "" >&2
echo "Eye $VERSION installed successfully." >&2
echo "Run 'eye --help' to get started." >&2
