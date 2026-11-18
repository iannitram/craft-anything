//! Regression tests for SquashFS v4.0 metadata table boundary calculation.

use craft_anything_formats::fsread::{FsKind, Limits, squashfs};
use craft_anything_testkit::firmware as fx;

/// Write a little-endian `u64` into `buf` at `offset`.
fn put_u64(buf: &mut [u8], offset: usize, value: u64) {
    buf[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

/// Standard SquashFS fixture with `inode_table < dir_table < id_table` continues to unpack cleanly.
#[test]
fn baseline_squashfs_fixture_reads_completely() {
    let img = squashfs::read(&fx::squashfs_image(), &Limits::default());
    assert!(img.complete && img.warnings.is_empty(), "{:?}", img.warnings);
    assert_eq!(img.entries.iter().find(|e| e.path == "readme").unwrap().data.as_deref(), Some(fx::SQUASHFS_README));
}

/// An NFS export lookup table (`offset 88`) or xattr table (`offset 56`) between `dir_table` and `id_table`
/// must bound `dir_table` so `Meta::read` does not overrun into raw `u64` pointer arrays.
#[test]
fn export_and_xattr_tables_before_id_table_do_not_corrupt_directory_scan() {
    let base = fx::squashfs_image();
    let old_id_table = u64::from_le_bytes(base[48..56].try_into().unwrap()) as usize;
    let id_tail = base[old_id_table..].to_vec();

    // Case 1: `mksquashfs` default NFS export table (`frag_table == u64::MAX`, `export_table` before `id_table`).
    // Layout after `dir_table`: [export metadata block (header 0x8008 + 8-byte inode ref)] [raw u64 export_table ptr] [id_table...]
    let mut with_export = base[..old_id_table].to_vec();
    let export_meta_off = with_export.len() as u64;
    with_export.extend_from_slice(&(8u16 | 0x8000).to_le_bytes());
    with_export.extend_from_slice(&0u64.to_le_bytes());
    let export_table_off = with_export.len() as u64;
    // Choose a metadata offset whose low 16 bits lack bit 15 (`0x8000`) so reading the raw u64 pointer
    // as a compressed gzip metadata header fails decompression if `dir_end` overruns `export_table`.
    with_export.extend_from_slice(&export_meta_off.to_le_bytes());
    let new_id_table = with_export.len() as u64;
    with_export.extend_from_slice(&id_tail);
    let new_id_block = new_id_table + 8;
    put_u64(&mut with_export, new_id_table as usize, new_id_block);
    let bytes_used = with_export.len() as u64;
    put_u64(&mut with_export, 40, bytes_used);
    put_u64(&mut with_export, 48, new_id_table);
    put_u64(&mut with_export, 88, export_table_off);

    let img_export = squashfs::read(&with_export, &Limits::default());
    assert!(
        img_export.complete && img_export.warnings.is_empty(),
        "valid SquashFS image with export_table must not report metadata overrun: {:?}",
        img_export.warnings
    );
    assert_eq!(img_export.entries.iter().find(|e| e.path == "etc/motd").unwrap().data.as_deref(), Some(b"welcome\n".as_slice()));

    // Case 2: `xattr_id_table_start` (`offset 56`) preceding `id_table` when `frag_table == u64::MAX`.
    let mut with_xattr = base[..old_id_table].to_vec();
    let xattr_table_off = with_xattr.len() as u64;
    // Raw 16-byte `squashfs_xattr_table` header starting with non-compressed-looking bytes (`0x0005`).
    with_xattr.extend_from_slice(&5u64.to_le_bytes());
    with_xattr.extend_from_slice(&0u64.to_le_bytes());
    let xattr_id_table = with_xattr.len() as u64;
    with_xattr.extend_from_slice(&id_tail);
    put_u64(&mut with_xattr, xattr_id_table as usize, xattr_id_table + 8);
    let used2 = with_xattr.len() as u64;
    put_u64(&mut with_xattr, 40, used2);
    put_u64(&mut with_xattr, 48, xattr_id_table);
    put_u64(&mut with_xattr, 56, xattr_table_off);

    let img_xattr = squashfs::read(&with_xattr, &Limits::default());
    assert!(
        img_xattr.complete && img_xattr.warnings.is_empty(),
        "xattr_id_table_start before id_table must bound directory metadata cleanly: {:?}",
        img_xattr.warnings
    );
    assert!(img_xattr.entries.iter().any(|e| e.path == "etc" && e.kind == FsKind::Dir));
}

/// Images where `dir_table` is the final table before `bytes_used` (or precedes `inode_table`) still unpack all files.
#[test]
fn reordered_tables_with_dir_table_at_end_or_before_inodes_unpack_files() {
    let base = fx::squashfs_image();
    let inode_off = u64::from_le_bytes(base[64..72].try_into().unwrap()) as usize;
    let dir_off = u64::from_le_bytes(base[72..80].try_into().unwrap()) as usize;
    let id_off = u64::from_le_bytes(base[48..56].try_into().unwrap()) as usize;

    let prefix = &base[..inode_off];
    let inode_bytes = &base[inode_off..dir_off];
    let dir_bytes = &base[dir_off..id_off];
    let id_bytes = &base[id_off..];

    // Layout: [superblock + data] [id_table] [dir_table] [inode_table] -> `inode_table` is last, `dir_table` precedes `inode_table`!
    let mut reordered = prefix.to_vec();
    let new_id = reordered.len() as u64;
    reordered.extend_from_slice(id_bytes);
    put_u64(&mut reordered, new_id as usize, new_id + 8);
    let new_dir = reordered.len() as u64;
    reordered.extend_from_slice(dir_bytes);
    let new_inode = reordered.len() as u64;
    reordered.extend_from_slice(inode_bytes);
    let used = reordered.len() as u64;

    put_u64(&mut reordered, 40, used);
    put_u64(&mut reordered, 48, new_id);
    put_u64(&mut reordered, 64, new_inode);
    put_u64(&mut reordered, 72, new_dir);

    let img = squashfs::read(&reordered, &Limits::default());
    assert!(img.complete && img.warnings.is_empty(), "{:?}", img.warnings);
    assert_eq!(img.entries.iter().find(|e| e.path == "readme").unwrap().data.as_deref(), Some(fx::SQUASHFS_README));

    // Also test `[superblock + data] [id_table] [inode_table] [dir_table]` where `dir_table` is the final table in the image.
    let mut dir_last = prefix.to_vec();
    let id2 = dir_last.len() as u64;
    dir_last.extend_from_slice(id_bytes);
    put_u64(&mut dir_last, id2 as usize, id2 + 8);
    let inode2 = dir_last.len() as u64;
    dir_last.extend_from_slice(inode_bytes);
    let dir2 = dir_last.len() as u64;
    dir_last.extend_from_slice(dir_bytes);
    let used2 = dir_last.len() as u64;

    put_u64(&mut dir_last, 40, used2);
    put_u64(&mut dir_last, 48, id2);
    put_u64(&mut dir_last, 64, inode2);
    put_u64(&mut dir_last, 72, dir2);

    let img_last = squashfs::read(&dir_last, &Limits::default());
    assert!(img_last.complete && img_last.warnings.is_empty(), "{:?}", img_last.warnings);
    assert_eq!(img_last.entries.iter().find(|e| e.path == "etc/motd").unwrap().data.as_deref(), Some(b"welcome\n".as_slice()));
}
