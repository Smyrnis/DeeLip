#!/usr/bin/env bash
# Detect the current Linux distro and install the matching prebuilt DeeLip
# package from the project's GitHub Releases (built by
# .github/workflows/package.yml) -- no local Rust toolchain or compiling
# required.
#
#   apt-based (Debian/Ubuntu/...)   -> .deb, installed via apt-get
#   dnf/yum-based (Fedora/RHEL/...) -> .rpm, installed via dnf/yum
#   zypper-based (openSUSE)         -> .rpm, installed via zypper
#   anything else (Arch, Alpine...) -> .tar.gz, unpacked into --prefix
#
# Usage: ./install.sh [--version=TAG] [--prefix=PATH] [--system]
#   --version=TAG   install a specific release tag instead of the latest
#   --prefix=PATH   install prefix for the .tar.gz fallback (default: see below)
#   --system        for the .tar.gz fallback, install to /usr/local instead
#                    of ~/.local; ignored for .deb/.rpm (they always install
#                    system-wide via the package manager)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

REPO="Smyrnis/DeeLip"
if git_url=$(git -C "$SCRIPT_DIR" remote get-url origin 2>/dev/null); then
    parsed="$(printf '%s' "$git_url" | sed -E 's#(git@|https://)github\.com[:/]([^/]+/[^/.]+)(\.git)?$#\2#')"
    [[ -n "$parsed" && "$parsed" != "$git_url" ]] && REPO="$parsed"
fi

VERSION=""
PREFIX=""
SYSTEM=0

for arg in "$@"; do
    case "$arg" in
        --version=*) VERSION="${arg#--version=}" ;;
        --prefix=*)  PREFIX="${arg#--prefix=}" ;;
        --system)    SYSTEM=1 ;;
        -h|--help)
            grep '^# ' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "Unknown option: $arg" >&2
            exit 1
            ;;
    esac
done

if [[ -z "$PREFIX" ]]; then
    PREFIX=$([[ "$SYSTEM" -eq 1 ]] && echo "/usr/local" || echo "$HOME/.local")
fi

SUDO=""
[[ "$(id -u)" -ne 0 ]] && SUDO="sudo"

ARCH="$(uname -m)"
if [[ "$ARCH" != "x86_64" ]]; then
    echo "error: no prebuilt DeeLip package for architecture '$ARCH' (only x86_64 is published)." >&2
    exit 1
fi

FETCH=""
if command -v curl >/dev/null 2>&1; then
    FETCH="curl -fsSL"
elif command -v wget >/dev/null 2>&1; then
    FETCH="wget -qO-"
else
    echo "error: need curl or wget to download a release." >&2
    exit 1
fi

# ── Pick the release to install ──────────────────────────────────────────────
if [[ -n "$VERSION" ]]; then
    api_url="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
else
    api_url="https://api.github.com/repos/$REPO/releases/latest"
fi

echo "==> Fetching release info from $REPO${VERSION:+ ($VERSION)}"
release_json="$($FETCH "$api_url")"
if [[ -z "$release_json" ]] || printf '%s' "$release_json" | grep -q '"message": *"Not Found"'; then
    echo "error: couldn't find a release${VERSION:+ tagged '$VERSION'} for $REPO." >&2
    exit 1
fi

find_asset_url() {
    # $1 = suffix to match, e.g. ".deb"
    printf '%s\n' "$release_json" \
        | grep -o '"browser_download_url": *"[^"]*"' \
        | sed -E 's/.*"(https:\/\/[^"]+)"/\1/' \
        | grep -F "$1" \
        | head -n1
}

download_to() {
    # $1 = url, $2 = destination path
    if [[ "$FETCH" == curl* ]]; then
        curl -fsSL -o "$2" "$1"
    else
        wget -qO "$2" "$1"
    fi
}

# ── Detect distro packaging family and install ───────────────────────────────
tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

if command -v apt-get >/dev/null 2>&1; then
    url="$(find_asset_url .deb)"
    [[ -z "$url" ]] && { echo "error: no .deb asset found in the release." >&2; exit 1; }
    echo "==> Downloading $(basename "$url")"
    download_to "$url" "$tmpdir/deelip.deb"
    echo "==> Installing via apt-get"
    $SUDO apt-get update
    $SUDO apt-get install -y "$tmpdir/deelip.deb"

elif command -v dnf >/dev/null 2>&1 || command -v yum >/dev/null 2>&1; then
    url="$(find_asset_url .rpm)"
    [[ -z "$url" ]] && { echo "error: no .rpm asset found in the release." >&2; exit 1; }
    echo "==> Downloading $(basename "$url")"
    download_to "$url" "$tmpdir/deelip.rpm"
    echo "==> Installing via $(command -v dnf >/dev/null 2>&1 && echo dnf || echo yum)"
    if command -v dnf >/dev/null 2>&1; then
        $SUDO dnf install -y "$tmpdir/deelip.rpm"
    else
        $SUDO yum install -y "$tmpdir/deelip.rpm"
    fi

elif command -v zypper >/dev/null 2>&1; then
    url="$(find_asset_url .rpm)"
    [[ -z "$url" ]] && { echo "error: no .rpm asset found in the release." >&2; exit 1; }
    echo "==> Downloading $(basename "$url")"
    download_to "$url" "$tmpdir/deelip.rpm"
    echo "==> Installing via zypper"
    $SUDO zypper --non-interactive install --allow-unsigned-rpm "$tmpdir/deelip.rpm"

else
    echo "==> No known package manager (apt/dnf/yum/zypper) found; falling back to the portable tar.gz"
    url="$(find_asset_url .tar.gz)"
    [[ -z "$url" ]] && { echo "error: no .tar.gz asset found in the release." >&2; exit 1; }
    echo "==> Downloading $(basename "$url")"
    download_to "$url" "$tmpdir/deelip.tar.gz"
    tar -xzf "$tmpdir/deelip.tar.gz" -C "$tmpdir"
    stage_dir="$(find "$tmpdir" -maxdepth 1 -mindepth 1 -type d | head -n1)"

    echo "==> Installing to $PREFIX"
    $SUDO install -Dm755 "$stage_dir/usr/bin/deelip" "$PREFIX/bin/deelip"
    $SUDO install -Dm644 "$stage_dir/usr/share/applications/deelip.desktop" "$PREFIX/share/applications/deelip.desktop"
    $SUDO install -Dm644 "$stage_dir/usr/share/icons/hicolor/256x256/apps/deelip.png" "$PREFIX/share/icons/hicolor/256x256/apps/deelip.png"

    command -v update-desktop-database >/dev/null 2>&1 && $SUDO update-desktop-database "$PREFIX/share/applications" || true
    command -v gtk-update-icon-cache >/dev/null 2>&1 && $SUDO gtk-update-icon-cache -f "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true

    case ":$PATH:" in
        *":$PREFIX/bin:"*) ;;
        *) echo "    Note: $PREFIX/bin is not on your PATH -- add it or run the binary by its full path." ;;
    esac
fi

echo "==> Done."
