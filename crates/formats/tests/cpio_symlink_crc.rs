use craft_anything_formats::tarcpio::cpio;
use craft_anything_testkit::firmware::cpio_newc;

/// Convert a single-member newc fixture to CRC and set the member checksum.
fn archive(mode: u32, checksum: u32) -> Vec<u8> {
    let mut bytes = cpio_newc(&[("entry", b"target", mode)]);
    bytes[..6].copy_from_slice(b"070702");
    bytes[102..110].copy_from_slice(format!("{checksum:08x}").as_bytes());
    let trailer = (110 + "entry".len() + 1 + 3) & !3;
    let trailer = (trailer + "target".len() + 3) & !3;
    bytes[trailer..trailer + 6].copy_from_slice(b"070702");
    bytes
}

/// GNU-style CRC symlinks carry a zero placeholder and keep the recorded target.
#[test]
fn accepts_a_symlink_with_no_stored_checksum() {
    let listing = cpio(&archive(0o120777, 0), 10).unwrap();
    assert!(listing.complete);
    assert_eq!(listing.members[0].link_target.as_deref(), Some("target"));
    assert!(listing.warnings.is_empty(), "{:?}", listing.warnings);
}

/// A nonzero symlink checksum and even a zero regular-file checksum are checked.
#[test]
fn still_checks_nonzero_symlink_and_zero_regular_file_checksums() {
    assert!(cpio(&archive(0o120777, 647), 10).unwrap().warnings.is_empty());
    for bytes in [archive(0o120777, 646), archive(0o100644, 0)] {
        assert!(cpio(&bytes, 10).unwrap().warnings.iter().any(|w| w.contains("checksum mismatch")));
    }
}
