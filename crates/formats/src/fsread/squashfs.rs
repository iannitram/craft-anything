//! A minimal read-only SquashFS v4.0 reader (little-endian), covering the common cases: gzip, xz,
//! zstd, lz4 and lzma compression (and uncompressed blocks), basic and extended directory/file inodes,
//! data blocks and tail fragments. It walks the tree from the root inode and extracts regular files
//! and fast symlinks. Bounded against cycles, huge sizes and corrupt references.
//!
//! NOT handled (reported in warnings): lzo compression, xattrs, and SquashFS versions other
//! than 4.0. These make affected files appear unrecovered rather than producing wrong bytes.

use super::{FsEntry, FsImage, FsKind, Limits, join};
use crate::bytes::Reader;
use std::collections::{BTreeMap, BTreeSet};

const META_MAX: usize = 8192;

struct Sb {
    block_size: u32,
    compression: u16,
    inode_table: u64,
    dir_table: u64,
    frag_table: u64,
    export_table: u64,
    id_table: u64,
    xattr_table: u64,
    root_ref: u64,
    frag_count: u32,
}

fn superblock(data: &[u8]) -> Option<Sb> {
    let r = Reader::new(data);
    if r.slice(0, 4) != Some(b"hsqs") || r.u16_le(28)? != 4 {
        return None;
    }
    let block_size = r.u32_le(12)?;
    let block_log = r.u16_le(22)?;
    if 1u32.checked_shl(u32::from(block_log)) != Some(block_size) || !(12..=20).contains(&block_log) {
        return None;
    }
    Some(Sb {
        block_size,
        compression: r.u16_le(20)?,
        frag_count: r.u32_le(16)?,
        root_ref: r.u64_le(32)?,
        inode_table: r.u64_le(64)?,
        dir_table: r.u64_le(72)?,
        frag_table: r.u64_le(80)?,
        export_table: r.u64_le(88)?,
        id_table: r.u64_le(48)?,
        xattr_table: r.u64_le(56)?,
    })
}

/// Decompress a metadata or data block with the superblock's compressor, capped at `max`.
fn decompress(comp: u16, block: &[u8], max: u64) -> Option<Vec<u8>> {
    match comp {
        1 => crate::firmware::inflate(block, true, max, true).ok().and_then(|i| i.data),
        2 => crate::decompress::lzma(block, max).ok(),
        4 => crate::decompress::xz_single(block, max).ok(),
        5 => usize::try_from(max).ok().and_then(|cap| crate::decompress::lz4_block_capped(block, cap).ok()),
        6 => crate::decompress::zstd(block, max).ok(),
        _ => None,
    }
}

/// A decompressed metadata region plus a map from each metadata block's start (relative to the
/// region start) to its position in the decompressed buffer.
struct Meta {
    data: Vec<u8>,
    map: BTreeMap<u32, usize>,
}

impl Meta {
    fn read(data: &[u8], start: u64, end: u64, comp: u16, warnings: &mut FsImage) -> Meta {
        let mut out = Vec::new();
        let mut map = BTreeMap::new();
        let (Ok(mut pos), Ok(end)) = (usize::try_from(start), usize::try_from(end)) else {
            return Meta { data: out, map };
        };
        let end = end.min(data.len());
        let r = Reader::new(data.get(..end).unwrap_or_default());
        let mut guard = 0usize;
        while pos < end {
            guard += 1;
            if guard > 100_000 || out.len() > 64 << 20 {
                break;
            }
            let block_start = out.len();
            let Some(hdr) = r.u16_le(pos) else {
                warnings.warn("squashfs: a metadata header exceeds its table");
                warnings.complete = false;
                break;
            };
            let uncompressed = hdr & 0x8000 != 0;
            let len = usize::from(hdr & 0x7fff);
            let Some(block) = r.slice(pos + 2, len) else {
                warnings.warn("squashfs: a metadata block exceeds its table");
                warnings.complete = false;
                break;
            };
            if uncompressed {
                out.extend_from_slice(block);
            } else if let Some(d) = decompress(comp, block, META_MAX as u64) {
                out.extend_from_slice(&d);
            } else {
                warnings.warn("squashfs: a metadata block did not decompress");
                warnings.complete = false;
                break;
            }
            map.insert((pos as u64 - start) as u32, block_start);
            pos += 2 + len;
        }
        Meta { data: out, map }
    }

    /// Resolve a metadata reference `(block_location << 16) | offset` to a decompressed position.
    fn at(&self, reference: u64) -> Option<usize> {
        let block = (reference >> 16) as u32;
        let off = (reference & 0xffff) as usize;
        self.map.get(&block).and_then(|&base| base.checked_add(off))
    }
}

pub fn read(data: &[u8], limits: &Limits) -> FsImage {
    let Some(sb) = superblock(data) else {
        let mut img = FsImage::new("squashfs");
        img.warn("not a SquashFS 4.0 image");
        img.complete = false;
        return img;
    };
    let mut img = FsImage::new("squashfs");
    let Some(used) = Reader::new(data).u64_le(40).filter(|&used| used >= 96) else {
        img.warn("squashfs: invalid filesystem size");
        img.complete = false;
        return img;
    };
    if used > data.len() as u64 {
        img.warn("squashfs: image ends before its declared size");
        img.complete = false;
    }
    let data = usize::try_from(used).ok().and_then(|end| data.get(..end)).unwrap_or(data);
    if sb.inode_table >= used || sb.dir_table > used {
        img.warn("squashfs: metadata table outside the filesystem");
        img.complete = false;
        return img;
    }
    if sb.compression == 3 {
        img.warn("squashfs uses lzo compression, which is not supported");
        img.complete = false;
    }
    // Each metadata table ends at the nearest following table header (or `used` when last).
    let tables = [sb.inode_table, sb.dir_table, sb.frag_table, sb.export_table, sb.id_table, sb.xattr_table];
    let table_end = |start: u64| tables.into_iter().filter(|&t| t > start && t != u64::MAX).min().unwrap_or(used).min(used);
    let inodes = Meta::read(data, sb.inode_table, table_end(sb.inode_table), sb.compression, &mut img);
    let dirs = Meta::read(data, sb.dir_table, table_end(sb.dir_table), sb.compression, &mut img);
    let mut seen = BTreeSet::new();
    walk(data, &sb, &inodes, &dirs, sb.root_ref, "", limits, &mut img, 0, &mut seen);
    img
}

#[allow(clippy::too_many_arguments)]
fn walk(
    data: &[u8],
    sb: &Sb,
    inodes: &Meta,
    dirs: &Meta,
    inode_ref: u64,
    parent: &str,
    limits: &Limits,
    img: &mut FsImage,
    depth: usize,
    seen: &mut BTreeSet<u64>,
) {
    if depth > limits.max_depth || img.entries.len() >= limits.max_entries {
        img.complete = false;
        return;
    }
    if !seen.insert(inode_ref) {
        return;
    }
    let r = Reader::new(&inodes.data);
    let Some((pos, itype)) = inodes.at(inode_ref).and_then(|pos| Some((pos, r.u16_le(pos)?))) else {
        img.warn("squashfs: directory inode not read");
        img.complete = false;
        return;
    };
    // Directory inode (basic=1, extended=8).
    let (start_block, size, offset) = match itype {
        1 => (r.u32_le(pos + 16), r.u16_le(pos + 24).map(u32::from), r.u16_le(pos + 26)),
        8 => (r.u32_le(pos + 24), r.u32_le(pos + 20), r.u16_le(pos + 34)),
        _ => return,
    };
    let (Some(start_block), Some(size), Some(offset)) = (start_block, size, offset) else { return };
    // The directory listing lives at (start_block, offset) in the directory table, `size`-3 bytes.
    let listing_len = size.saturating_sub(3) as usize;
    if listing_len == 0 {
        return;
    }
    let listing = dirs.at((u64::from(start_block) << 16) | u64::from(offset)).and_then(|start| dirs.data.get(start..)).unwrap_or_default();
    let listing = listing.get(..listing_len).unwrap_or_else(|| {
        img.warn("squashfs: incomplete directory listing");
        img.complete = false;
        listing
    });
    parse_dir(data, sb, inodes, dirs, listing, parent, limits, img, depth, seen);
}

#[allow(clippy::too_many_arguments)]
fn parse_dir(
    data: &[u8],
    sb: &Sb,
    inodes: &Meta,
    dirs: &Meta,
    listing: &[u8],
    parent: &str,
    limits: &Limits,
    img: &mut FsImage,
    depth: usize,
    seen: &mut BTreeSet<u64>,
) {
    let r = Reader::new(listing);
    let mut pos = 0usize;
    while pos + 12 <= listing.len() {
        let count = r.u32_le(pos).unwrap_or(0);
        let block_start = r.u32_le(pos + 4).unwrap_or(0);
        pos += 12;
        if count >= 256 {
            break;
        }
        for _ in 0..=count {
            if img.entries.len() >= limits.max_entries {
                img.complete = false;
                return;
            }
            let Some(entry_off) = r.u16_le(pos) else { return };
            let name_len = r.u16_le(pos + 6).map(|v| usize::from(v) + 1).unwrap_or(0);
            let etype = r.u16_le(pos + 4).unwrap_or(0);
            let name = listing.get(pos + 8..pos + 8 + name_len).map(|b| String::from_utf8_lossy(b).into_owned()).unwrap_or_default();
            pos += 8 + name_len;
            if name.is_empty() || name_len > limits.max_name {
                continue;
            }
            let child_ref = (u64::from(block_start) << 16) | u64::from(entry_off);
            let path = join(parent, &name);
            // Entry type: 1=dir, 2=file, 3=symlink (and 8/9/10 extended variants).
            match etype {
                1 | 8 => {
                    img.entries.push(FsEntry::dir(path.clone()));
                    walk(data, sb, inodes, dirs, child_ref, &path, limits, img, depth + 1, seen);
                }
                2 | 9 => {
                    let bytes = inodes.at(child_ref).and_then(|p| file_bytes(data, sb, &inodes.data, p, limits, img));
                    let size = bytes.as_ref().map(|b| b.len() as u64).unwrap_or(0);
                    img.entries.push(FsEntry::file(path, size, bytes));
                }
                3 | 10 => {
                    let target = inodes.at(child_ref).and_then(|p| symlink_target(&inodes.data, p)).unwrap_or_default();
                    img.entries.push(FsEntry::symlink(path, target));
                }
                _ => img.entries.push(FsEntry { path, kind: FsKind::Other, size: 0, symlink_target: None, data: None }),
            }
        }
    }
}

fn symlink_target(idata: &[u8], pos: usize) -> Option<String> {
    let r = Reader::new(idata);
    let len = r.u32_le(pos + 20)? as usize;
    idata.get(pos + 24..pos + 24 + len.min(4096)).map(|b| String::from_utf8_lossy(b).into_owned())
}

/// Read a regular file's bytes: its data blocks (from the archive body) plus any tail fragment.
fn file_bytes(data: &[u8], sb: &Sb, idata: &[u8], pos: usize, limits: &Limits, img: &mut FsImage) -> Option<Vec<u8>> {
    let r = Reader::new(idata);
    let itype = r.u16_le(pos)?;
    // Regular file inode (basic=2, extended=9).
    let (blocks_start, fragment, frag_offset, file_size, list_off) = match itype {
        2 => (u64::from(r.u32_le(pos + 16)?), r.u32_le(pos + 20)?, r.u32_le(pos + 24)?, u64::from(r.u32_le(pos + 28)?), pos + 32),
        9 => {
            let xattr = r.u32_le(pos + 52)?;
            if xattr != 0xffff_ffff {
                img.warn("squashfs: extended inode xattrs are not supported");
                img.complete = false;
            }
            (r.u64_le(pos + 16)?, r.u32_le(pos + 44)?, r.u32_le(pos + 48)?, r.u64_le(pos + 24)?, pos + 56)
        }
        _ => return None,
    };
    if file_size > limits.max_file_bytes || img.bytes.saturating_add(file_size) > limits.max_total_bytes {
        img.warn("squashfs: file skipped (size budget)");
        img.complete = false;
        return None;
    }
    let bs = u64::from(sb.block_size);
    let has_fragment = fragment != 0xffff_ffff;
    let full_blocks = if has_fragment { file_size / bs } else { file_size.div_ceil(bs) };
    if full_blocks > 1 << 20 {
        img.warn("squashfs: file block count exceeds recovery limit");
        img.complete = false;
        return None;
    }
    let mut out = Vec::with_capacity(usize::try_from(file_size).unwrap_or(0));
    let mut data_pos = blocks_start;
    for i in 0..usize::try_from(full_blocks).ok()? {
        let expected = usize::try_from(file_size.saturating_sub(i as u64 * bs).min(bs)).ok()?;
        let Some(bsize) = r.u32_le(list_off + i * 4) else {
            img.warn("squashfs: file block list not read");
            img.complete = false;
            return None;
        };
        let uncompressed = bsize & 0x0100_0000 != 0;
        let len = (bsize & 0x00ff_ffff) as usize;
        if len == 0 {
            // Sparse block.
            out.resize(out.len().saturating_add(expected), 0);
            continue;
        }
        let Some(block) = usize::try_from(data_pos).ok().and_then(|start| Reader::new(data).slice(start, len)) else {
            img.warn("squashfs: file block outside the filesystem");
            img.complete = false;
            return None;
        };
        if uncompressed {
            if block.len() != expected {
                img.warn("squashfs: data block length does not match its logical file span");
                img.complete = false;
                return None;
            }
            out.extend_from_slice(block);
        } else {
            match decompress(sb.compression, block, expected as u64) {
                Some(d) if d.len() == expected => out.extend_from_slice(&d),
                _ => {
                    img.warn("squashfs: data block did not decode to its logical file span");
                    img.complete = false;
                    return None;
                }
            }
        }
        data_pos = data_pos.checked_add(len as u64)?;
    }
    if has_fragment {
        match fragment_bytes(data, sb, fragment, img) {
            Some(frag) => {
                let tail = file_size.saturating_sub(out.len() as u64) as usize;
                let o = frag_offset as usize;
                let Some(slice) = frag.get(o..o.checked_add(tail)?) else {
                    img.warn("squashfs: fragment not read");
                    img.complete = false;
                    return None;
                };
                out.extend_from_slice(slice);
            }
            None => {
                img.warn("squashfs: fragment not read");
                img.complete = false;
                return None;
            }
        }
    }
    out.truncate(usize::try_from(file_size).unwrap_or(0));
    img.bytes = img.bytes.saturating_add(out.len() as u64);
    Some(out)
}

/// Resolve and decompress a fragment block.
fn fragment_bytes(data: &[u8], sb: &Sb, index: u32, img: &mut FsImage) -> Option<Vec<u8>> {
    if index >= sb.frag_count {
        return None;
    }
    let r = Reader::new(data);
    // The fragment table start is an array of u64 metadata-block pointers.
    let per_block = META_MAX / 16; // fragment entries per metadata block
    let which = (index as usize) / per_block;
    let within = (index as usize) % per_block;
    let ptr_off = usize::try_from(sb.frag_table).ok()?.checked_add(which.checked_mul(8)?)?;
    let meta_block = r.u64_le(ptr_off)?;
    let mut tmp = FsImage::new("squashfs");
    // Decompress just this one metadata block.
    let mstart = usize::try_from(meta_block).ok()?;
    let hdr = r.u16_le(mstart)?;
    let uncompressed = hdr & 0x8000 != 0;
    let len = usize::from(hdr & 0x7fff);
    let block = data.get(mstart + 2..mstart + 2 + len)?;
    let meta = if uncompressed { block.to_vec() } else { decompress(sb.compression, block, META_MAX as u64)? };
    let _ = &mut tmp;
    let e = within.checked_mul(16)?;
    let mr = Reader::new(&meta);
    let frag_start = mr.u64_le(e)?;
    let frag_size = mr.u32_le(e + 8)?;
    let comp = frag_size & 0x0100_0000 == 0;
    let flen = (frag_size & 0x00ff_ffff) as usize;
    let fstart = usize::try_from(frag_start).ok()?;
    let fblock = data.get(fstart..fstart.checked_add(flen)?)?;
    if comp {
        decompress(sb.compression, fblock, u64::from(sb.block_size)).or_else(|| {
            img.complete = false;
            None
        })
    } else {
        Some(fblock.to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xz_container_blocks_do_not_concatenate_streams() {
        use craft_anything_testkit::firmware as fx;
        let concatenated = [fx::XZ_FIXTURE, fx::XZ_FIXTURE].concat();
        assert_eq!(decompress(4, fx::XZ_FIXTURE, fx::XZ_PLAIN.len() as u64).as_deref(), Some(fx::XZ_PLAIN));
        assert!(decompress(4, fx::XZ_FIXTURE, fx::XZ_PLAIN.len() as u64 - 1).is_none());
        assert_eq!(decompress(4, &concatenated, fx::XZ_PLAIN.len() as u64).as_deref(), Some(fx::XZ_PLAIN));
        assert_eq!(crate::decompress::xz(&concatenated, 2 * fx::XZ_PLAIN.len() as u64).unwrap(), [fx::XZ_PLAIN, fx::XZ_PLAIN].concat());
    }

    #[test]
    fn truncated_metadata_preserves_only_preceding_blocks() {
        // Cut the second block inside its two-byte header and inside its payload.
        let data = [1, 0x80, b'a', 2, 0x80, b'b', b'c'];
        for (end, warning) in [(3, None), (4, Some("squashfs: a metadata header exceeds its table")), (6, Some("squashfs: a metadata block exceeds its table"))]
        {
            let mut img = FsImage::new("squashfs");
            let meta = Meta::read(&data, 0, end, 1, &mut img);
            assert_eq!(meta.data, b"a");
            assert_eq!(meta.at(0), Some(0));
            assert!(meta.at(3 << 16).is_none());
            assert_eq!(img.complete, warning.is_none(), "table end {end}");
            assert_eq!(img.warnings.first().map(String::as_str), warning, "table end {end}");
        }
    }

    #[test]
    fn reads_a_small_image() {
        let img = read(&craft_anything_testkit::firmware::squashfs_image(), &Limits::default());
        assert!(img.complete, "{:?}", img.warnings);
        assert_eq!(img.entries.iter().find(|e| e.path == "readme").unwrap().data.as_deref(), Some(craft_anything_testkit::firmware::SQUASHFS_README));
        assert!(img.entries.iter().any(|e| e.path == "etc" && e.kind == FsKind::Dir));
        assert_eq!(img.entries.iter().find(|e| e.path == "etc/motd").unwrap().data.as_deref(), Some(b"welcome\n".as_slice()));
    }

    #[test]
    fn reads_extended_regular_file_inodes() {
        let sb = Sb {
            block_size: 4096,
            compression: 1,
            inode_table: 0,
            dir_table: 0,
            frag_table: 0,
            export_table: u64::MAX,
            id_table: 0,
            xattr_table: u64::MAX,
            root_ref: 0,
            frag_count: 0,
        };
        let body = b"extended file payload";
        let mut data = vec![0u8; 96];
        data.extend_from_slice(body);
        let mut idata = Vec::new();
        idata.extend_from_slice(&9u16.to_le_bytes());
        idata.extend_from_slice(&0o644u16.to_le_bytes());
        idata.resize(16, 0);
        idata.extend_from_slice(&96u64.to_le_bytes());
        idata.extend_from_slice(&(body.len() as u64).to_le_bytes());
        idata.extend_from_slice(&0u64.to_le_bytes());
        idata.extend_from_slice(&2u32.to_le_bytes());
        idata.extend_from_slice(&0xffff_ffffu32.to_le_bytes());
        idata.extend_from_slice(&0u32.to_le_bytes());
        idata.extend_from_slice(&0xffff_ffffu32.to_le_bytes());
        idata.extend_from_slice(&((body.len() as u32) | 0x0100_0000).to_le_bytes());
        let mut img = FsImage::new("squashfs");
        let got = file_bytes(&data, &sb, &idata, 0, &Limits::default(), &mut img);
        assert_eq!(got.as_deref(), Some(body.as_slice()));
        assert!(img.complete, "{:?}", img.warnings);

        let mut bad_frag = idata.clone();
        bad_frag[44..48].copy_from_slice(&0u32.to_le_bytes());
        let mut fail_img = FsImage::new("squashfs");
        assert!(file_bytes(&data, &sb, &bad_frag, 0, &Limits::default(), &mut fail_img).is_none());
        assert!(!fail_img.complete);
    }

    #[test]
    fn hostile_never_panics() {
        let base = craft_anything_testkit::firmware::squashfs_image();
        for m in craft_anything_testkit::mutations(&base) {
            let _ = read(&m, &Limits::default());
        }
        for len in (0..base.len()).step_by(5) {
            let _ = read(&base[..len], &Limits::default());
        }
        for seed in 0..16 {
            let _ = read(&craft_anything_testkit::noise(seed, 2048), &Limits::default());
        }
    }
}
