#!/usr/bin/env bash
# Shared helpers for install.sh/uninstall.sh/health-check.sh. Sourced, not
# executed directly -- every function prints its result to stdout and
# signals failure via a non-zero exit status, so callers capture with
# `x="$(fn args)"` the same way throughout.

# $1 = a directory inside the repo (or anywhere, for a non-clone). Prints the
# GitHub "owner/repo" these scripts' releases live under -- sniffed from the
# caller's own git remote when run from a clone, since the same scripts also
# ship inside a release tar.gz (no .git present there) and could be run from
# a fork publishing releases under a different name; falls back to the
# canonical repo when it can't be sniffed.
lib_detect_repo() {
    local dir="$1" default="Smyrnis/DeeLip" git_url parsed
    if git_url=$(git -C "$dir" remote get-url origin 2>/dev/null); then
        parsed="$(printf '%s' "$git_url" | sed -E 's#(git@|https://)github\.com[:/]([^/]+/[^/.]+)(\.git)?$#\2#')"
        if [[ -n "$parsed" && "$parsed" != "$git_url" ]]; then
            printf '%s\n' "$parsed"
            return 0
        fi
    fi
    printf '%s\n' "$default"
}

# Prints "curl -fsSL" or "wget -qO-" (a fetch-to-stdout command prefix), or
# fails if neither is available.
lib_detect_fetch() {
    if command -v curl >/dev/null 2>&1; then
        printf '%s\n' "curl -fsSL"
    elif command -v wget >/dev/null 2>&1; then
        printf '%s\n' "wget -qO-"
    else
        echo "error: need curl or wget." >&2
        return 1
    fi
}

# Prints "sudo" when not already root, else nothing.
lib_detect_sudo() {
    [[ "$(id -u)" -ne 0 ]] && printf '%s\n' "sudo" || printf '%s\n' ""
}

# Prints one of apt/dnf/yum/zypper/none -- same precedence install.sh's own
# distro detection has always used.
lib_detect_pkg_family() {
    if command -v apt-get >/dev/null 2>&1; then
        printf '%s\n' "apt"
    elif command -v dnf >/dev/null 2>&1; then
        printf '%s\n' "dnf"
    elif command -v yum >/dev/null 2>&1; then
        printf '%s\n' "yum"
    elif command -v zypper >/dev/null 2>&1; then
        printf '%s\n' "zypper"
    else
        printf '%s\n' "none"
    fi
}

# $1 = repo ("owner/name"), $2 = version tag (empty = latest), $3 = fetch
# command (from lib_detect_fetch). Prints the release's raw JSON.
lib_fetch_release_json() {
    local repo="$1" version="$2" fetch="$3" api_url json
    if [[ -n "$version" ]]; then
        api_url="https://api.github.com/repos/$repo/releases/tags/$version"
    else
        api_url="https://api.github.com/repos/$repo/releases/latest"
    fi
    json="$($fetch "$api_url")"
    if [[ -z "$json" ]] || printf '%s' "$json" | grep -q '"message": *"Not Found"'; then
        echo "error: couldn't find a release${version:+ tagged '$version'} for $repo." >&2
        return 1
    fi
    printf '%s\n' "$json"
}

# $1 = release JSON (from lib_fetch_release_json), $2 = suffix to match
# (e.g. ".deb"). Prints the first matching asset's download URL, if any.
lib_find_asset_url() {
    # The `|| true` matters under `set -e`: callers do
    # `url="$(lib_find_asset_url ...)"` then check `[[ -z "$url" ]]`
    # themselves for a friendly "no matching asset" error -- without it, a
    # release genuinely missing this asset type makes `grep -F` fail, which
    # (via `pipefail`) fails this whole function, which aborts the *caller*
    # via `set -e` before it ever reaches its own error message.
    printf '%s\n' "$1" \
        | grep -o '"browser_download_url": *"[^"]*"' \
        | sed -E 's/.*"(https:\/\/[^"]+)"/\1/' \
        | { grep -F "$2" || true; } \
        | head -n1
}

# $1 = fetch command (from lib_detect_fetch), $2 = url, $3 = destination path.
lib_download_to() {
    if [[ "$1" == curl* ]]; then
        curl -fsSL -o "$3" "$2"
    else
        wget -qO "$3" "$2"
    fi
}

# $1 = 1 if --system was passed, else 0. Prints the default install prefix
# for the tar.gz fallback path.
lib_resolve_prefix() {
    if [[ "$1" -eq 1 ]]; then
        printf '%s\n' "/usr/local"
    else
        printf '%s\n' "$HOME/.local"
    fi
}

# $1 = a .desktop file's Exec= value (e.g. "deelip" or a full path, possibly
# with trailing %f/%u-style field codes no DeeLip .desktop file actually
# uses, but handled anyway since it costs nothing). Prints the resolved
# executable's path, or nothing if it can't be resolved at all.
lib_resolve_exec_binary() {
    local exec_value="${1%% *}"
    if [[ "$exec_value" == /* ]]; then
        printf '%s\n' "$exec_value"
    else
        command -v "$exec_value" 2>/dev/null || true
    fi
}
