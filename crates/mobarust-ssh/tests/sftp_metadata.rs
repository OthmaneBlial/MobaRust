//! File-type codes are exclusive POSIX values, not independent flags.

use russh_sftp::protocol::{FileAttributes, FileMode, FileType};

#[test]
fn file_type_predicates_match_exact_type_codes() {
    for (mode, expected) in [
        (0o040000, Some("directory")),
        (0o100000, Some("regular")),
        (0o120000, Some("symlink")),
        (0o020000, Some("character")),
        (0o060000, Some("block")),
        (0o010000, Some("fifo")),
        (0o140000, None),
        (0o170000, None),
        (0, None),
    ] {
        let mut attrs = FileAttributes::empty();
        attrs.permissions = Some(mode | 0o4751);
        let matches = [
            ("directory", attrs.is_dir()),
            ("regular", attrs.is_regular()),
            ("symlink", attrs.is_symlink()),
            ("character", attrs.is_character()),
            ("block", attrs.is_block()),
            ("fifo", attrs.is_fifo()),
        ]
        .into_iter()
        .filter_map(|(name, matches)| matches.then_some(name))
        .collect::<Vec<_>>();
        assert_eq!(
            matches,
            expected.into_iter().collect::<Vec<_>>(),
            "mode {mode:o}"
        );
    }
    let absent = FileAttributes::empty();
    assert!(!absent.is_regular() && !absent.is_dir() && !absent.is_symlink());
}

#[test]
fn type_setters_replace_the_type_and_preserve_permission_bits() {
    let mut attrs = FileAttributes::empty();
    attrs.permissions = Some(0o100640);
    attrs.set_symlink(true);
    assert_eq!(attrs.permissions, Some(0o120640));
    attrs.set_regular(false);
    assert_eq!(
        attrs.permissions,
        Some(0o120640),
        "clearing regular must not corrupt a symlink"
    );
    attrs.set_dir(true);
    assert_eq!(attrs.permissions, Some(0o040640));
    attrs.set_block(true);
    assert_eq!(attrs.permissions, Some(0o060640));
    attrs.set_character(false);
    assert_eq!(
        attrs.permissions,
        Some(0o060640),
        "clearing character must not corrupt a block device"
    );
    attrs.remove_type(FileMode::DIR);
    assert_eq!(attrs.permissions, Some(0o060640));
    attrs.remove_type(FileMode::BLK);
    assert_eq!(attrs.permissions, Some(0o640));
    assert_eq!(attrs.file_type(), FileType::Other);
}

#[cfg(unix)]
#[test]
fn unix_metadata_conversion_preserves_actual_file_types() {
    use std::os::unix::fs::MetadataExt;
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("file");
    let subdirectory = directory.path().join("directory");
    let link = directory.path().join("link");
    let socket = directory.path().join("socket");
    std::fs::write(&file, b"private fixture").unwrap();
    std::fs::create_dir(&subdirectory).unwrap();
    std::os::unix::fs::symlink(&file, &link).unwrap();
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    for path in [&file, &subdirectory, &link, &socket] {
        let metadata = std::fs::symlink_metadata(path).unwrap();
        let attrs = FileAttributes::from(&metadata);
        assert_eq!(attrs.permissions, Some(metadata.mode()));
        assert_eq!(attrs.is_dir(), metadata.is_dir());
        assert_eq!(attrs.is_regular(), metadata.is_file());
        assert_eq!(attrs.is_symlink(), metadata.file_type().is_symlink());
    }
    drop(listener);
}
