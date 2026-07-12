#!/usr/bin/env bash
# Detect the current Linux distro and install the matching prebuilt DeeLip
# package from the project's GitHub Releases (built by
# .github/workflows/package.yml) -- no local Rust toolchain or compiling
# required. Shared helpers live in helpers/lib.sh alongside this file.
#
#   apt-based (Debian/Ubuntu/...)   -> .deb, installed via apt-get
#   dnf/yum-based (Fedora/RHEL/...) -> .rpm, installed via dnf/yum
#   zypper-based (openSUSE)         -> .rpm, installed via zypper
#   anything else (Arch, Alpine...) -> .tar.gz, unpacked into --prefix
#
# Usage: scripts/install.sh [--version=TAG] [--prefix=PATH] [--system]
#   --version=TAG   install a specific release tag instead of the latest
#   --prefix=PATH   install prefix for the .tar.gz fallback (default: see below)
#   --system        for the .tar.gz fallback, install to /usr/local instead
#                    of ~/.local; ignored for .deb/.rpm (they always install
#                    system-wide via the package manager)
set -euo pipefail

usage() {
    cat <<'EOF'
Detect the current Linux distro and install the matching prebuilt DeeLip
package from the project's GitHub Releases (built by
.github/workflows/package.yml) -- no local Rust toolchain or compiling
required. Shared helpers live in helpers/lib.sh alongside this file, or are
fetched over the network if run via curl | bash (see DEELIP_SCRIPTS_REF below).

  apt-based (Debian/Ubuntu/...)   -> .deb, installed via apt-get
  dnf/yum-based (Fedora/RHEL/...) -> .rpm, installed via dnf/yum
  zypper-based (openSUSE)         -> .rpm, installed via zypper
  anything else (Arch, Alpine...) -> .tar.gz, unpacked into --prefix

Usage: scripts/install.sh [--version=TAG] [--prefix=PATH] [--system]
  --version=TAG   install a specific release tag instead of the latest
  --prefix=PATH   install prefix for the .tar.gz fallback (default: see below)
  --system        for the .tar.gz fallback, install to /usr/local instead
                   of ~/.local; ignored for .deb/.rpm (they always install
                   system-wide via the package manager)

Env:
  DEELIP_SCRIPTS_REF  when run via curl | bash, which branch/tag to fetch
                      helpers/lib.sh from (default: main)
EOF
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]:-}")" 2>/dev/null && pwd || true)"
DEELIP_SCRIPTS_REF="${DEELIP_SCRIPTS_REF:-main}"

if [[ -n "$SCRIPT_DIR" && -f "$SCRIPT_DIR/helpers/lib.sh" ]]; then
    # shellcheck source=./helpers/lib.sh
    source "$SCRIPT_DIR/helpers/lib.sh"
else
    # No local checkout/tarball alongside this file (curl | bash) -- fetch the
    # same helpers over the network. Pin DEELIP_SCRIPTS_REF=<tag> if you
    # fetched a specific release's script rather than main's.
    #
    # Also overwrite SCRIPT_DIR: bash leaves BASH_SOURCE[0]/$0 as a bare
    # word (e.g. "bash") when a script comes from stdin rather than empty,
    # so the `dirname`/`cd`/`pwd` above silently resolved to the *current
    # working directory* instead of failing -- which could be some unrelated
    # git checkout the user happened to be sitting in. Point lib_detect_repo
    # (below) at a directory that can't possibly have its own git remote to
    # accidentally sniff, so it reliably falls back to the hardcoded default.
    SCRIPT_DIR="/nonexistent/deelip-curl-bash"
    lib_url="https://raw.githubusercontent.com/Smyrnis/DeeLip/$DEELIP_SCRIPTS_REF/scripts/helpers/lib.sh"
    if command -v curl >/dev/null 2>&1; then
        lib_src="$(curl -fsSL "$lib_url")" || { echo "error: couldn't fetch $lib_url" >&2; exit 1; }
    elif command -v wget >/dev/null 2>&1; then
        lib_src="$(wget -qO- "$lib_url")" || { echo "error: couldn't fetch $lib_url" >&2; exit 1; }
    else
        echo "error: need curl or wget to fetch scripts/helpers/lib.sh" >&2
        exit 1
    fi
    [[ -z "$lib_src" ]] && { echo "error: fetched empty content from $lib_url" >&2; exit 1; }
    eval "$lib_src"
fi

REPO="$(lib_detect_repo "$SCRIPT_DIR")"

VERSION=""
PREFIX=""
SYSTEM=0

for arg in "$@"; do
    case "$arg" in
        --version=*) VERSION="${arg#--version=}" ;;
        --prefix=*)  PREFIX="${arg#--prefix=}" ;;
        --system)    SYSTEM=1 ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $arg" >&2
            exit 1
            ;;
    esac
done

[[ -z "$PREFIX" ]] && PREFIX="$(lib_resolve_prefix "$SYSTEM")"

SUDO="$(lib_detect_sudo)"

ARCH="$(uname -m)"
if [[ "$ARCH" != "x86_64" ]]; then
    echo "error: no prebuilt DeeLip package for architecture '$ARCH' (only x86_64 is published)." >&2
    exit 1
fi

FETCH="$(lib_detect_fetch)"

echo "==> Fetching release info from $REPO${VERSION:+ ($VERSION)}"
release_json="$(lib_fetch_release_json "$REPO" "$VERSION" "$FETCH")"

tmpdir="$(mktemp -d)"
trap 'rm -rf "$tmpdir"' EXIT

family="$(lib_detect_pkg_family)"
case "$family" in
    apt)
        url="$(lib_find_asset_url "$release_json" .deb)"
        [[ -z "$url" ]] && { echo "error: no .deb asset found in the release." >&2; exit 1; }
        echo "==> Downloading $(basename "$url")"
        lib_download_to "$FETCH" "$url" "$tmpdir/deelip.deb"
        echo "==> Installing via apt-get"
        $SUDO apt-get update
        $SUDO apt-get install -y "$tmpdir/deelip.deb"
        ;;

    dnf|yum)
        url="$(lib_find_asset_url "$release_json" .rpm)"
        [[ -z "$url" ]] && { echo "error: no .rpm asset found in the release." >&2; exit 1; }
        echo "==> Downloading $(basename "$url")"
        lib_download_to "$FETCH" "$url" "$tmpdir/deelip.rpm"
        echo "==> Installing via $family"
        $SUDO "$family" install -y "$tmpdir/deelip.rpm"
        ;;

    zypper)
        url="$(lib_find_asset_url "$release_json" .rpm)"
        [[ -z "$url" ]] && { echo "error: no .rpm asset found in the release." >&2; exit 1; }
        echo "==> Downloading $(basename "$url")"
        lib_download_to "$FETCH" "$url" "$tmpdir/deelip.rpm"
        echo "==> Installing via zypper"
        $SUDO zypper --non-interactive install --allow-unsigned-rpm "$tmpdir/deelip.rpm"
        ;;

    none)
        echo "==> No known package manager (apt/dnf/yum/zypper) found; falling back to the portable tar.gz"
        url="$(lib_find_asset_url "$release_json" .tar.gz)"
        [[ -z "$url" ]] && { echo "error: no .tar.gz asset found in the release." >&2; exit 1; }
        echo "==> Downloading $(basename "$url")"
        lib_download_to "$FETCH" "$url" "$tmpdir/deelip.tar.gz"
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
        ;;
esac

echo "==> Done."
