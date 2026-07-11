#!/usr/bin/env bash
# Reverses whichever install path scripts/install.sh took -- package-manager
# remove for .deb/.rpm installs, or deleting the specific files the tar.gz
# fallback placed. Never touches ~/.config/deelip/ (db, recordings, logs,
# crash reports) unless --purge is given.
#
# Usage: scripts/uninstall.sh [--prefix=PATH] [--system] [--purge] [-y|--yes]
#   --prefix=PATH  prefix to remove the tar.gz fallback's files from
#                  (default: same as install.sh's -- ~/.local, or /usr/local
#                  with --system)
#   --system       look under /usr/local instead of ~/.local for the tar.gz
#                  fallback path; ignored for .deb/.rpm (removed via their
#                  own package manager regardless of this flag)
#   --purge        also remove ~/.config/deelip/ (real user data: accounts,
#                  contacts, history, recordings, logs, crash reports) and
#                  the XDG autostart entry -- prompts for confirmation
#                  unless -y/--yes is given
#   -y, --yes      don't prompt for confirmation (needed for --purge to run
#                  non-interactively)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./helpers/lib.sh
source "$SCRIPT_DIR/helpers/lib.sh"

PREFIX=""
SYSTEM=0
PURGE=0
YES=0

for arg in "$@"; do
    case "$arg" in
        --prefix=*) PREFIX="${arg#--prefix=}" ;;
        --system)   SYSTEM=1 ;;
        --purge)    PURGE=1 ;;
        -y|--yes)   YES=1 ;;
        -h|--help)
            # Prints just the leading comment block (the usage text above),
            # not every "# ..." comment in the whole file -- stops at the
            # first non-comment line rather than grepping the file broadly.
            awk '/^#!/{next} /^# ?/{sub(/^# ?/,""); print; next} {exit}' "$0"
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

confirm() {
    # $1 = prompt. Succeeds if the user (or -y/--yes) confirms.
    [[ "$YES" -eq 1 ]] && return 0
    local reply
    read -r -p "$1 [y/N] " reply
    [[ "$reply" =~ ^[Yy]$ ]]
}

removed_anything=0

if dpkg -s deelip >/dev/null 2>&1; then
    echo "==> Removing the apt-installed deelip package"
    $SUDO apt-get remove -y deelip
    removed_anything=1
elif rpm -q deelip >/dev/null 2>&1; then
    echo "==> Removing the rpm-installed deelip package"
    if command -v dnf >/dev/null 2>&1; then
        $SUDO dnf remove -y deelip
    elif command -v yum >/dev/null 2>&1; then
        $SUDO yum remove -y deelip
    else
        $SUDO zypper --non-interactive remove deelip
    fi
    removed_anything=1
else
    bin="$PREFIX/bin/deelip"
    desktop="$PREFIX/share/applications/deelip.desktop"
    icon="$PREFIX/share/icons/hicolor/256x256/apps/deelip.png"
    found=0
    for f in "$bin" "$desktop" "$icon"; do
        if [[ -e "$f" ]]; then
            found=1
            echo "==> Removing $f"
            $SUDO rm -f "$f"
        fi
    done
    if [[ "$found" -eq 1 ]]; then
        removed_anything=1
        command -v update-desktop-database >/dev/null 2>&1 && $SUDO update-desktop-database "$PREFIX/share/applications" || true
        command -v gtk-update-icon-cache >/dev/null 2>&1 && $SUDO gtk-update-icon-cache -f "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true
    else
        echo "==> No manually-installed DeeLip files found under $PREFIX"
        echo "    (pass --prefix=PATH or --system if it was installed somewhere else)"
    fi
fi

autostart="$HOME/.config/autostart/deelip.desktop"
if [[ -e "$autostart" ]]; then
    echo "==> Removing the autostart entry ($autostart)"
    rm -f "$autostart"
    removed_anything=1
fi

if [[ "$removed_anything" -eq 0 ]]; then
    echo "==> Nothing found to uninstall."
fi

if [[ "$PURGE" -eq 1 ]]; then
    data_dir="$HOME/.config/deelip"
    if [[ -d "$data_dir" ]]; then
        if confirm "Also delete $data_dir (accounts, contacts, history, recordings, logs)? This cannot be undone."; then
            echo "==> Removing $data_dir"
            rm -rf "$data_dir"
        else
            echo "==> Keeping $data_dir"
        fi
    fi
fi

echo "==> Done."
