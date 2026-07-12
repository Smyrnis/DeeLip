# Uninstall

`scripts/uninstall.sh` reverses whichever path [`install.sh`](/docs/install/install) took —
`apt-get`/`dnf`/`yum`/`zypper remove` for a package-manager install, or deleting the specific
files the `.tar.gz` fallback placed.

```sh
./scripts/uninstall.sh
```

Same one-liner form as `install.sh`, if you don't have a local checkout:

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Smyrnis/DeeLip/main/scripts/uninstall.sh)"
```

For `--purge`, either pass `-y`/`--yes` or use this same `bash -c "$(curl ...)"` form (not a
plain `curl ... | bash` pipe) — a raw pipe leaves no real stdin for the confirmation prompt to
read, so it silently defaults to "no":

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Smyrnis/DeeLip/main/scripts/uninstall.sh)" -- --purge -y
```

By default it **never touches** `~/.config/deelip/` — your accounts, contacts, call history,
recordings, and logs are left alone. It does remove the XDG autostart entry
(`~/.config/autostart/deelip.desktop`) if one exists, since that's written by DeeLip itself
rather than owned by any package manager.

## Flags

```
scripts/uninstall.sh [--prefix=PATH] [--system] [--purge] [-y|--yes]
```

- `--prefix=PATH` — prefix to remove the `.tar.gz` fallback's files from (default: same
  resolution as `install.sh` — `~/.local`, or `/usr/local` with `--system`). Ignored for
  `.deb`/`.rpm` installs, which are always removed via their own package manager.
- `--system` — look under `/usr/local` instead of `~/.local` for the `.tar.gz` fallback path.
- `--purge` — also delete `~/.config/deelip/` (real user data: accounts, contacts, history,
  recordings, logs, crash reports). Prompts for confirmation unless `-y`/`--yes` is given.
- `-y`, `--yes` — don't prompt for confirmation. Required for `--purge` to run non-interactively.

## Reinstalling instead of uninstalling

If DeeLip is misbehaving but you don't actually want it gone, try
[`scripts/health-check.sh --fix`](/docs/install/health-check) first — it repairs a broken
install without touching your data, which `uninstall.sh` followed by a fresh `install.sh` also
does, just with more steps.
