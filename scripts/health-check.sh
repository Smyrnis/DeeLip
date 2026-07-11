#!/usr/bin/env bash
# Verifies the pieces install.sh may have placed are still intact, and (with
# --fix) repairs whichever ones aren't -- for when a user (or some cleanup
# tool) has deleted a file DeeLip depends on existing outside its own
# control: the binary, its desktop-launcher entry, its icon, or the XDG
# autostart entry if that's enabled. Deliberately does NOT check
# ~/.config/deelip/ (the SQLite db, recordings, logs, crash reports) -- those
# are already self-healing (every table is created with
# `CREATE TABLE IF NOT EXISTS` on load), so there's nothing here to "fix."
#
# Usage: scripts/health-check.sh [--prefix=PATH] [--system] [--fix] [--version=TAG]
#   --prefix=PATH  where to look for a tar.gz-fallback install (default:
#                  same as install.sh's -- ~/.local, or /usr/local with
#                  --system)
#   --system       look under /usr/local instead of ~/.local
#   --fix          attempt to repair anything found broken (without it, this
#                  script only reports problems and changes nothing)
#   --version=TAG  when repairing a tar.gz install, fetch this release
#                  instead of the latest
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=./helpers/lib.sh
source "$SCRIPT_DIR/helpers/lib.sh"

PREFIX=""
SYSTEM=0
FIX=0
VERSION=""

for arg in "$@"; do
    case "$arg" in
        --prefix=*)  PREFIX="${arg#--prefix=}" ;;
        --system)    SYSTEM=1 ;;
        --fix)       FIX=1 ;;
        --version=*) VERSION="${arg#--version=}" ;;
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

# Populates bin_path/desktop/icon (empty if missing/broken) and the
# `problems` count, printing one status line per check. Re-run after a
# repair attempt to confirm it actually worked, rather than assuming it did.
run_checks() {
    problems=0
    bin_path=""
    desktop=""
    icon=""

    if command -v deelip >/dev/null 2>&1; then
        bin_path="$(command -v deelip)"
        echo "[ok]   deelip binary found on PATH: $bin_path"
    else
        echo "[FAIL] deelip binary not found on PATH"
        problems=$((problems + 1))
    fi

    # /usr/... covers a .deb/.rpm install (never affected by --prefix);
    # $PREFIX/... covers the tar.gz fallback, at whatever prefix it was
    # actually installed to -- $PREFIX already defaults to $HOME/.local or
    # /usr/local (see lib_resolve_prefix), but --prefix=PATH accepts
    # anything, so this must follow it rather than guess from a fixed list.
    for candidate in \
        "/usr/share/applications/deelip.desktop" \
        "$PREFIX/share/applications/deelip.desktop"
    do
        if [[ -e "$candidate" ]]; then
            desktop="$candidate"
            break
        fi
    done
    if [[ -z "$desktop" ]]; then
        echo "[FAIL] no deelip.desktop launcher entry found in any standard location"
        problems=$((problems + 1))
    else
        exec_line="$(grep -m1 '^Exec=' "$desktop" | cut -d= -f2-)" || true
        exec_bin="$(lib_resolve_exec_binary "$exec_line")"
        if [[ -n "$exec_bin" && -x "$exec_bin" ]]; then
            echo "[ok]   desktop entry found ($desktop), Exec= resolves to a real binary"
        else
            echo "[FAIL] desktop entry found ($desktop), but its Exec= ('$exec_line') doesn't resolve to a real binary"
            problems=$((problems + 1))
        fi
    fi

    for candidate in \
        "/usr/share/icons/hicolor/256x256/apps/deelip.png" \
        "$PREFIX/share/icons/hicolor/256x256/apps/deelip.png"
    do
        if [[ -e "$candidate" ]]; then
            icon="$candidate"
            break
        fi
    done
    if [[ -z "$icon" ]]; then
        echo "[FAIL] no deelip app icon found in any standard location"
        problems=$((problems + 1))
    else
        echo "[ok]   app icon found: $icon"
    fi

    autostart="$HOME/.config/autostart/deelip.desktop"
    if [[ -e "$autostart" ]]; then
        exec_line="$(grep -m1 '^Exec=' "$autostart" | cut -d= -f2-)" || true
        exec_bin="$(lib_resolve_exec_binary "$exec_line")"
        if [[ -n "$exec_bin" && -x "$exec_bin" ]]; then
            echo "[ok]   autostart entry found ($autostart), Exec= resolves to a real binary"
        else
            echo "[FAIL] autostart entry found ($autostart), but its Exec= ('$exec_line') doesn't resolve to a real binary"
            problems=$((problems + 1))
        fi
    else
        echo "[--]   autostart not enabled (no $autostart) -- not a problem, it's opt-in"
    fi
}

echo "==> Checking DeeLip's installed pieces..."
run_checks

if [[ "$problems" -eq 0 ]]; then
    echo
    echo "==> Everything looks healthy."
    exit 0
fi

echo
echo "==> Found $problems problem(s)."
if [[ "$FIX" -ne 1 ]]; then
    echo "    Re-run with --fix to attempt repair."
    exit 1
fi

echo "==> Attempting repair..."
family="$(lib_detect_pkg_family)"
SUDO="$(lib_detect_sudo)"

case "$family" in
    apt)
        echo "==> Reinstalling via apt-get (the package manager owns these files)"
        $SUDO apt-get install --reinstall -y deelip
        ;;
    dnf)
        $SUDO dnf reinstall -y deelip
        ;;
    yum)
        $SUDO yum reinstall -y deelip
        ;;
    zypper)
        $SUDO zypper --non-interactive install --force deelip
        ;;
    none)
        echo "==> No package manager owns this install -- re-fetching the tar.gz release to restore missing files"
        REPO="$(lib_detect_repo "$SCRIPT_DIR")"
        FETCH="$(lib_detect_fetch)"
        release_json="$(lib_fetch_release_json "$REPO" "$VERSION" "$FETCH")"
        url="$(lib_find_asset_url "$release_json" .tar.gz)"
        [[ -z "$url" ]] && { echo "error: no .tar.gz asset found in the release." >&2; exit 1; }

        tmpdir="$(mktemp -d)"
        trap 'rm -rf "$tmpdir"' EXIT
        echo "==> Downloading $(basename "$url")"
        lib_download_to "$FETCH" "$url" "$tmpdir/deelip.tar.gz"
        tar -xzf "$tmpdir/deelip.tar.gz" -C "$tmpdir"
        stage_dir="$(find "$tmpdir" -maxdepth 1 -mindepth 1 -type d | head -n1)"

        if [[ -z "$bin_path" ]]; then
            echo "==> Restoring $PREFIX/bin/deelip"
            $SUDO install -Dm755 "$stage_dir/usr/bin/deelip" "$PREFIX/bin/deelip"
        fi
        if [[ -z "$desktop" ]]; then
            echo "==> Restoring $PREFIX/share/applications/deelip.desktop"
            $SUDO install -Dm644 "$stage_dir/usr/share/applications/deelip.desktop" "$PREFIX/share/applications/deelip.desktop"
        fi
        if [[ -z "$icon" ]]; then
            echo "==> Restoring $PREFIX/share/icons/hicolor/256x256/apps/deelip.png"
            $SUDO install -Dm644 "$stage_dir/usr/share/icons/hicolor/256x256/apps/deelip.png" "$PREFIX/share/icons/hicolor/256x256/apps/deelip.png"
        fi
        command -v update-desktop-database >/dev/null 2>&1 && $SUDO update-desktop-database "$PREFIX/share/applications" || true
        command -v gtk-update-icon-cache >/dev/null 2>&1 && $SUDO gtk-update-icon-cache -f "$PREFIX/share/icons/hicolor" >/dev/null 2>&1 || true
        ;;
esac

# The autostart entry isn't owned by any package manager (it's written by
# DeelipApp's own Settings toggle, see crates/config/src/autostart.rs) --
# if it was broken, repoint it at whichever binary path just got fixed above
# rather than leaving it stale.
if [[ -e "$autostart" ]]; then
    exec_line="$(grep -m1 '^Exec=' "$autostart" | cut -d= -f2-)" || true
    exec_bin="$(lib_resolve_exec_binary "$exec_line")"
    if [[ -z "$exec_bin" || ! -x "$exec_bin" ]]; then
        new_bin="$bin_path"
        [[ -z "$new_bin" ]] && new_bin="$PREFIX/bin/deelip"
        if [[ -x "$new_bin" ]]; then
            echo "==> Repointing the autostart entry at $new_bin"
            sed -i "s#^Exec=.*#Exec=$new_bin#" "$autostart"
        fi
    fi
fi

echo
echo "==> Re-checking..."
run_checks

if [[ "$problems" -eq 0 ]]; then
    echo
    echo "==> Repair succeeded -- everything looks healthy now."
else
    echo
    echo "==> $problems problem(s) remain after repair -- may need a full scripts/install.sh re-run."
    exit 1
fi
