use craft_anything_formats::tarcpio::{MemberKind, member_data, tar};
use craft_anything_testkit::firmware::tar_entries;

/// Build an old GNU sparse header followed by the requested number of extension blocks.
fn archive(extension_blocks: usize) -> Vec<u8> {
    let mut archive = tar_entries(&[("sparse.dat", b'S', &[b'x'; 512], ""), ("after.txt", b'0', b"after", "")]);
    archive[257..265].copy_from_slice(b"ustar  \0");
    archive[482] = u8::from(extension_blocks != 0);
    archive[148..156].fill(b' ');
    let sum: u32 = archive[..512].iter().map(|&byte| u32::from(byte)).sum();
    archive[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    let mut extensions = vec![0; 512 * extension_blocks];
    for block in extensions.as_chunks_mut::<512>().0.iter_mut().take(extension_blocks.saturating_sub(1)) {
        block[504] = 1;
    }
    archive.splice(512..512, extensions);
    archive
}

/// Actual file data starts after the entire sparse map. Later members remain readable.
#[test]
fn skips_old_gnu_sparse_extension_blocks() {
    for blocks in [1, 2] {
        let bytes = archive(blocks);
        let listing = tar(&bytes, 8).unwrap();
        assert!(listing.complete, "{blocks} extensions: {:?}", listing.warnings);
        assert_eq!(listing.members.len(), 2);
        assert_eq!(listing.members[0].kind, MemberKind::Other);
        assert_eq!(listing.members[0].data_offset, (512 * (blocks + 1)) as u64);
        assert_eq!(listing.members[1].name, "after.txt");
        assert_eq!(member_data(&bytes, &listing.members[1]), Some(b"after".as_slice()));
    }
}

/// A missing sparse extension cannot turn padding into a complete archive.
#[test]
fn refuses_a_truncated_old_gnu_sparse_extension() {
    let mut bytes = archive(2);
    bytes.truncate(512 + 512 + 17);
    let listing = tar(&bytes, 8).unwrap();
    assert!(!listing.complete);
    assert!(listing.members.is_empty());
    assert!(listing.warnings.iter().any(|warning| warning.contains("sparse extension")));
}

/// A sparse member with no extensions still leaves the next header at its usual offset.
#[test]
fn keeps_the_next_member_when_no_sparse_extensions_follow() {
    let bytes = archive(0);
    let listing = tar(&bytes, 8).unwrap();
    assert!(listing.complete);
    assert_eq!(listing.members[0].kind, MemberKind::Other);
    assert_eq!(listing.members[1].name, "after.txt");
}
