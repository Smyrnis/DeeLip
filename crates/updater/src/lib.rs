//! Checks the project's GitHub Releases for a newer DeeLip version and,
//! when the running binary is user-writable (the portable tar.gz/
//! `install.sh` path), replaces it in place. System package installs
//! (.deb/.rpm) are deliberately never touched here -- see
//! [`can_self_replace`]'s doc comment -- those are left to the user's
//! package manager, same as `install.sh` itself does.

use std::path::{Path, PathBuf};

use anyhow::Context;
use serde::Deserialize;

/// The GitHub `owner/repo` slug releases are checked against.
pub const REPO: &str = "Smyrnis/DeeLip";

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag:      String,
    pub version:  String,
    pub html_url: String,
    tar_gz_url:   Option<String>,
}

#[derive(Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Deserialize)]
struct GhRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    assets:   Vec<GhAsset>,
}

/// Fetches the latest published release and returns it if its version is
/// newer than `current` -- `None` either if we're already up to date or if
/// the tag doesn't parse as semver (treated as "nothing to report" rather
/// than an error, since a hand-pushed non-version tag shouldn't nag the user).
pub fn check_latest(current: &str) -> anyhow::Result<Option<ReleaseInfo>> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let body = ureq::get(&url)
        .header("User-Agent", "deelip-updater")
        .call()
        .context("Requesting latest release")?
        .body_mut()
        .read_to_string()
        .context("Reading release response")?;
    let release: GhRelease = serde_json::from_str(&body).context("Parsing release JSON")?;

    let Some(version) = newer_version(&release.tag_name, current) else { return Ok(None) };

    let tar_gz_url = release.assets.iter()
        .find(|a| a.name.ends_with(".tar.gz"))
        .map(|a| a.browser_download_url.clone());

    Ok(Some(ReleaseInfo { tag: release.tag_name, version, html_url: release.html_url, tar_gz_url }))
}

/// Returns `tag`'s version string (leading `v` stripped) if it parses as
/// semver and is strictly newer than `current` -- `None` both when it's not
/// newer and when either string fails to parse (a hand-pushed non-version
/// tag shouldn't error the whole check, just report nothing to do).
fn newer_version(tag: &str, current: &str) -> Option<String> {
    let version = tag.trim_start_matches('v');
    let latest = semver::Version::parse(version).ok()?;
    let current = semver::Version::parse(current).ok()?;
    (latest > current).then(|| version.to_string())
}

/// Whether the running binary can be updated in place without elevated
/// privileges -- true for a `~/.local/bin`-style user install (what
/// `install.sh`'s tar.gz fallback produces), false for a system package.
/// `.deb`/`.rpm` installs put the binary under `/usr/bin`, owned by (and not
/// writable outside of) dpkg/rpm's package database -- overwriting it
/// directly would desync that database from what's actually on disk, so
/// those installs are only ever offered a link to the release page instead.
///
/// Only the *directory*'s write permission matters here, not the exe file's
/// own: `download_and_replace` never opens the running binary for writing
/// (Linux refuses that with ETXTBSY while it's executing) -- it stages the
/// new binary alongside it and `rename()`s over it, which is a directory
/// operation and works on a currently-running executable just fine.
pub fn can_self_replace() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let Some(dir) = exe.parent() else { return false };
    dir_is_writable(dir)
}

fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".deelip-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => { let _ = std::fs::remove_file(&probe); true }
        Err(_) => false,
    }
}

/// Downloads `release`'s `.tar.gz` asset, extracts the `deelip` binary from
/// it, and atomically swaps it in for the currently running executable.
/// Only meaningful (and only ever called) when `can_self_replace()` is true.
///
/// Linux allows replacing/unlinking the file backing an already-running
/// process -- this process keeps executing fine off its old (now-unlinked)
/// inode until it next exits, so there's no need to stop anything first;
/// the *next* launch is what actually picks up the new binary. Callers are
/// expected to prompt the user to restart rather than doing it
/// automatically (an in-progress call would otherwise be dropped).
pub fn download_and_replace(release: &ReleaseInfo) -> anyhow::Result<()> {
    let url = release.tar_gz_url.as_deref()
        .ok_or_else(|| anyhow::anyhow!("Release {} has no .tar.gz asset", release.tag))?;

    let mut res = ureq::get(url)
        .header("User-Agent", "deelip-updater")
        .call()
        .context("Downloading update")?;

    let tmp_dir = std::env::temp_dir().join(format!("deelip-update-{}", std::process::id()));
    std::fs::create_dir_all(&tmp_dir).context("Creating temp dir for update")?;

    let archive_path = tmp_dir.join("deelip.tar.gz");
    {
        let mut file = std::fs::File::create(&archive_path).context("Creating temp archive file")?;
        std::io::copy(&mut res.body_mut().as_reader(), &mut file).context("Saving downloaded archive")?;
    }

    let current_exe = std::env::current_exe().context("Locating running executable")?;
    let result = install_from_archive(&archive_path, &current_exe);
    let _ = std::fs::remove_dir_all(&tmp_dir);
    result
}

/// The pure extract-and-swap-in half of [`download_and_replace`], split out
/// so it's testable against a hand-built local archive/target instead of a
/// real network download and the real running executable.
fn install_from_archive(archive_path: &Path, current_exe: &Path) -> anyhow::Result<()> {
    let extract_dir = archive_path.with_extension("");
    std::fs::create_dir_all(&extract_dir).context("Creating extraction dir")?;

    let tar_gz = std::fs::File::open(archive_path).context("Opening downloaded archive")?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tar_gz));
    archive.unpack(&extract_dir).context("Extracting update archive")?;

    let new_binary = find_named(&extract_dir, "deelip")
        .context("Update archive did not contain the deelip binary")?;

    // Staged in the *same* directory as the real binary (not the temp
    // extraction dir, which may be a different filesystem) so the final
    // rename is an atomic same-filesystem move rather than a cross-
    // filesystem copy.
    let staged = current_exe.with_extension("new");
    std::fs::copy(&new_binary, &staged).context("Staging new binary")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .context("Setting new binary's permissions")?;
    }
    std::fs::rename(&staged, current_exe).context("Installing new binary")?;
    Ok(())
}

fn find_named(dir: &Path, name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_named(&path, name) { return Some(found); }
        } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
            return Some(path);
        }
    }
    None
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
