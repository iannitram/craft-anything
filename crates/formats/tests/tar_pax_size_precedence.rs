use craft_anything_formats::tarcpio::{member_data, tar};
use craft_anything_testkit::firmware::tar_entries;

/// Make an octal size unreadable while leaving the enclosing TAR header checksum valid.
fn corrupt_size(archive: &mut [u8], offset: usize) {
    let header = &mut archive[offset..offset + 512];
    header[124] = b'9';
    header[148..156].fill(b' ');
    let sum: u32 = header.iter().map(|&byte| u32::from(byte)).sum();
    header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
}

/// Local and inherited size records override the bad field and keep the following header aligned.
#[test]
fn valid_pax_size_controls_the_member_extent() {
    for extension in *b"xg" {
        let mut archive = tar_entries(&[("PaxHeader", extension, b"10 size=3\n", ""), ("file", b'0', b"abc", ""), ("next", b'0', b"def", "")]);
        corrupt_size(&mut archive, 1024);
        let list = tar(&archive, 10).unwrap();
        assert!(list.complete, "{:?}", list.warnings);
        assert_eq!(list.members.len(), 2);
        assert_eq!(member_data(&archive, &list.members[0]), Some(b"abc".as_slice()));
        assert_eq!(member_data(&archive, &list.members[1]), Some(b"def".as_slice()));
    }
}

/// A local zero overrides a global size and does not consume the next header.
#[test]
fn a_local_zero_size_overrides_the_global_value() {
    let mut archive =
        tar_entries(&[("Global", b'g', b"12 size=600\n", ""), ("Local", b'x', b"10 size=0\n", ""), ("empty", b'0', b"", ""), ("next", b'0', &[b'a'; 600], "")]);
    corrupt_size(&mut archive, 2048);
    let list = tar(&archive, 10).unwrap();
    assert!(list.complete, "{:?}", list.warnings);
    assert_eq!(list.members.iter().map(|m| m.size).collect::<Vec<_>>(), [0, 600]);
}

/// An ordinary header without an override and a PAX header itself still need readable sizes.
#[test]
fn rejects_bad_sizes_without_an_applicable_override() {
    let mut plain = tar_entries(&[("file", b'0', b"abc", "")]);
    corrupt_size(&mut plain, 0);
    let mut with_global = tar_entries(&[("Global", b'g', b"10 size=3\n", ""), ("Local", b'x', b"10 size=0\n", ""), ("file", b'0', b"", "")]);
    corrupt_size(&mut with_global, 1024);
    for archive in [plain, with_global] {
        let list = tar(&archive, 10).unwrap();
        assert!(!list.complete);
        assert!(list.members.is_empty());
        assert!(list.warnings.iter().any(|warning| warning.contains("unreadable size")));
    }
}
