use super::*;

#[test]
fn newer_version_reports_a_higher_patch() {
    assert_eq!(newer_version("v0.1.1", "0.1.0"), Some("0.1.1".to_string()));
}

#[test]
fn newer_version_ignores_equal_or_older() {
    assert_eq!(newer_version("v0.1.0", "0.1.0"), None);
    assert_eq!(newer_version("v0.0.9", "0.1.0"), None);
}

#[test]
fn newer_version_tolerates_a_bare_tag_without_v_prefix() {
    assert_eq!(newer_version("2.0.0", "1.9.9"), Some("2.0.0".to_string()));
}

#[test]
fn newer_version_treats_a_non_semver_tag_as_nothing_to_report() {
    assert_eq!(newer_version("nightly-build", "0.1.0"), None);
}

#[test]
fn find_named_locates_a_nested_file() {
    let dir = tempfile::tempdir().unwrap();
    let nested = dir.path().join("usr").join("bin");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("deelip"), b"binary content").unwrap();

    let found = find_named(dir.path(), "deelip").unwrap();
    assert_eq!(found, nested.join("deelip"));
}

#[test]
fn find_named_returns_none_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("readme.txt"), b"nothing here").unwrap();
    assert!(find_named(dir.path(), "deelip").is_none());
}

/// Builds a `.tar.gz` at `archive_path` containing a single file at
/// `usr/bin/deelip` with `contents` -- mirrors the layout `package.yml`'s
/// tar.gz step actually produces.
fn build_fixture_archive(archive_path: &std::path::Path, contents: &[u8]) {
    let tar_gz = std::fs::File::create(archive_path).unwrap();
    let encoder = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);

    let mut header = tar::Header::new_gnu();
    header.set_size(contents.len() as u64);
    header.set_mode(0o755);
    header.set_cksum();
    builder
        .append_data(&mut header, "usr/bin/deelip", contents)
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap();
}

#[test]
fn install_from_archive_swaps_in_the_new_binary() {
    let dir = tempfile::tempdir().unwrap();
    let archive_path = dir.path().join("update.tar.gz");
    build_fixture_archive(&archive_path, b"new binary bytes");

    let current_exe = dir.path().join("deelip");
    std::fs::write(&current_exe, b"old binary bytes").unwrap();

    install_from_archive(&archive_path, &current_exe).unwrap();

    let installed = std::fs::read(&current_exe).unwrap();
    assert_eq!(installed, b"new binary bytes");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&current_exe)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o755);
    }
}

#[test]
fn parse_sha256sums_finds_the_matching_line() {
    let body = "aaaa111  deelip-0.1.0-x86_64-linux.tar.gz\nbbbb222  deelip-0.1.0-x86_64.AppImage\n";
    assert_eq!(
        parse_sha256sums(body, "deelip-0.1.0-x86_64-linux.tar.gz"),
        Some("aaaa111".to_string())
    );
}

#[test]
fn parse_sha256sums_tolerates_binary_mode_asterisk() {
    let body = "aaaa111 *deelip-0.1.0-x86_64-linux.tar.gz\n";
    assert_eq!(
        parse_sha256sums(body, "deelip-0.1.0-x86_64-linux.tar.gz"),
        Some("aaaa111".to_string())
    );
}

#[test]
fn parse_sha256sums_is_case_insensitive_on_hash_but_not_filename() {
    let body = "AAAA111  deelip-0.1.0-x86_64-linux.tar.gz\n";
    assert_eq!(
        parse_sha256sums(body, "deelip-0.1.0-x86_64-linux.tar.gz"),
        Some("aaaa111".to_string())
    );
}

#[test]
fn parse_sha256sums_returns_none_when_no_line_matches() {
    let body = "aaaa111  some-other-file.tar.gz\n";
    assert_eq!(parse_sha256sums(body, "deelip-0.1.0-x86_64-linux.tar.gz"), None);
}

#[test]
fn verify_checksum_accepts_a_matching_hash() {
    use sha2::{Digest, Sha256};

    let dir = tempfile::tempdir().unwrap();
    let archive_path = dir.path().join("update.tar.gz");
    build_fixture_archive(&archive_path, b"new binary bytes");

    let real_hash = {
        let mut hasher = Sha256::new();
        hasher.update(std::fs::read(&archive_path).unwrap());
        format!("{:x}", hasher.finalize())
    };

    verify_checksum(&archive_path, Some(&real_hash)).unwrap();
}

#[test]
fn verify_checksum_rejects_a_mismatched_hash() {
    let dir = tempfile::tempdir().unwrap();
    let archive_path = dir.path().join("update.tar.gz");
    build_fixture_archive(&archive_path, b"new binary bytes");

    let err = verify_checksum(&archive_path, Some("0000000000000000000000000000000000000000000000000000000000000000"))
        .unwrap_err();
    assert!(err.to_string().contains("Checksum mismatch"));
}

#[test]
fn verify_checksum_proceeds_when_none_published() {
    let dir = tempfile::tempdir().unwrap();
    let archive_path = dir.path().join("update.tar.gz");
    build_fixture_archive(&archive_path, b"new binary bytes");

    verify_checksum(&archive_path, None).unwrap();
}

#[test]
fn install_from_archive_errors_when_binary_missing_from_tarball() {
    let dir = tempfile::tempdir().unwrap();
    let archive_path = dir.path().join("update.tar.gz");

    // A tarball with some other file, not `deelip`, anywhere in it.
    let tar_gz = std::fs::File::create(&archive_path).unwrap();
    let encoder = flate2::write::GzEncoder::new(tar_gz, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    let mut header = tar::Header::new_gnu();
    header.set_size(5);
    header.set_mode(0o644);
    header.set_cksum();
    builder
        .append_data(&mut header, "usr/share/doc/deelip/copyright", &b"hello"[..])
        .unwrap();
    builder.into_inner().unwrap().finish().unwrap();

    let current_exe = dir.path().join("deelip");
    std::fs::write(&current_exe, b"old binary bytes").unwrap();

    let err = install_from_archive(&archive_path, &current_exe).unwrap_err();
    assert!(err
        .to_string()
        .contains("did not contain the deelip binary"));
    // The old binary must be left untouched on failure.
    assert_eq!(std::fs::read(&current_exe).unwrap(), b"old binary bytes");
}
