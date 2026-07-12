# DeeLip
Softphone

## Install

```sh
bash -c "$(curl -fsSL https://raw.githubusercontent.com/Smyrnis/DeeLip/main/scripts/install.sh)"
```

Detects your distro's package manager (`apt`/`dnf`/`yum`/`zypper`) and installs the matching
prebuilt package from GitHub Releases, or falls back to a portable `.tar.gz` for anything else.
See the [docs](https://smyrnis.github.io/DeeLip/) for flags, uninstalling, and everything else.

On Windows or macOS, grab the `.msi`/`.dmg` from the
[latest release](https://github.com/Smyrnis/DeeLip/releases/latest) instead — see
[Downloads](https://smyrnis.github.io/DeeLip/downloads/) for details.
