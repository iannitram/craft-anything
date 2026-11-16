//! ZIP archives (also APK, JAR, IPA, MSIX, HAP…): central directory listing and bounded entry reads.
//!
//! Only the central directory is trusted for the listing. Entry data is located through the local
//! header, decompressed with a hard output cap, and checked against the recorded CRC-32. Stored,
//! DEFLATE, bzip2, ZIP LZMA, XZ and Zstandard entries are supported; other methods and encrypted entries are listed.

use crate::bytes::Reader;

/// Biggest tail we search for the end-of-central-directory record (64 KiB comment + record).
pub(crate) const EOCD_SEARCH: usize = 22 + 0xffff;
const ZIP64_FIXED_SIZE: usize = 56;
/// Bound the search for a record shifted by a prepended stub, including extensible data.
const ZIP64_RECOVERY_SEARCH: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ZipError {
    #[error("not a ZIP archive (no end-of-central-directory record)")]
    NotZip,
    #[error("ZIP is truncated: {0}")]
    Truncated(&'static str),
    #[error("ZIP is corrupt: {0}")]
    Corrupt(String),
    #[error("unsupported ZIP feature: {0}")]
    Unsupported(String),
    #[error("entry is larger than the {0}-byte limit")]
    TooLarge(u64),
    #[error("CRC-32 mismatch (expected {expected:08x}, found {found:08x})")]
    CrcMismatch { expected: u32, found: u32 },
}

/// One central-directory entry.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ZipEntry {
    /// Entry name as stored (forward slashes). Bit 11 selects lossy UTF-8; otherwise
    /// a valid Unicode Path extra field takes precedence over the UTF-8/CP437 fallback.
    pub name: String,
    pub method: u16,
    pub flags: u16,
    pub crc32: u32,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    /// Offset of the local header, already adjusted for any prepended data.
    pub local_header_offset: u64,
    pub is_dir: bool,
    pub encrypted: bool,
    /// Unix mode from the external attributes (when stored by Unix or macOS).
    #[serde(skip)]
    pub unix_mode: Option<u32>,
}

impl ZipEntry {
    /// Why this entry is not a regular file or directory (symlink, device, FIFO, socket), if so.
    pub fn special_kind(&self) -> Option<&'static str> {
        match self.unix_mode? & 0o170000 {
            0o120000 => Some("symlink"),
            0o020000 => Some("character device"),
            0o060000 => Some("block device"),
            0o010000 => Some("FIFO"),
            0o140000 => Some("socket"),
            _ => None,
        }
    }

    /// Human name of the compression method.
    pub fn method_name(&self) -> &'static str {
        match self.method {
            0 => "stored",
            8 => "deflate",
            9 => "deflate64",
            12 => "bzip2",
            14 => "lzma",
            93 => "zstd",
            95 => "xz",
            99 => "aes",
            _ => "other",
        }
    }
}

/// The parsed central directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ZipArchive {
    pub entries: Vec<ZipEntry>,
    /// Entries the directory declares (may exceed `entries.len()` when truncated).
    pub declared_entries: u64,
    /// True when `max_entries` stopped the listing early.
    pub truncated: bool,
    pub zip64: bool,
    /// Bytes before the archive proper (self-extracting stubs).
    pub prefix_bytes: u64,
    /// Inconsistent end-record metadata retained without hiding a readable directory.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

/// True when the bytes start like a ZIP (local header or empty archive).
pub fn looks_like_zip(data: &[u8]) -> bool {
    data.starts_with(b"PK\x03\x04") || data.starts_with(b"PK\x05\x06") || data.starts_with(b"PK\x07\x08")
}

/// Recover a shifted end record only when its fixed fields still describe the classic
/// directory. Longer records must frame their extension exactly; the adjacent fixed
/// record can still report an inconsistent size through the normal warning path.
fn recover_zip64_record(data: &[u8], loc: usize, disk_entries: u64, total: u64, cd_size: u64, cd_offset: u64) -> Option<usize> {
    let r = Reader::new(data);
    let last = loc.checked_sub(ZIP64_FIXED_SIZE)?;
    let first = loc.saturating_sub(ZIP64_RECOVERY_SEARCH);
    let candidates = data.get(first..last.checked_add(4)?)?;
    candidates.windows(4).enumerate().rev().find_map(|(relative, signature)| {
        if signature != b"PK\x06\x06" {
            return None;
        }
        let rec = first.checked_add(relative)?;
        let fixed = Reader::new(r.slice(rec, ZIP64_FIXED_SIZE)?);
        let payload_size = fixed.u64_le(4)?;
        if payload_size < 44 {
            return None;
        }
        let declared_end = (rec as u64).checked_add(12)?.checked_add(payload_size)?;
        if rec != last && declared_end != loc as u64 {
            return None;
        }
        let record_disk_entries = fixed.u64_le(24)?;
        let record_total = fixed.u64_le(32)?;
        let record_cd_size = fixed.u64_le(40)?;
        let record_cd_offset = fixed.u64_le(48)?;
        if fixed.u32_le(16)? != 0
            || fixed.u32_le(20)? != 0
            || record_disk_entries != record_total
            || (disk_entries != 0xffff && disk_entries != record_disk_entries)
            || (total != 0xffff && total != record_total)
            || (cd_size != 0xffff_ffff && cd_size != record_cd_size)
            || (cd_offset != 0xffff_ffff && cd_offset != record_cd_offset)
            || record_cd_offset.checked_add(record_cd_size)? > rec as u64
        {
            return None;
        }
        if record_total > 0 {
            let directory = rec.checked_sub(usize::try_from(record_cd_size).ok()?)?;
            if record_cd_size < 46 || r.slice(directory, 4) != Some(b"PK\x01\x02") {
                return None;
            }
        }
        Some(rec)
    })
}

/// Parse the central directory, keeping at most `max_entries`.
pub fn parse(data: &[u8], max_entries: usize) -> Result<ZipArchive, ZipError> {
    parse_directory(data, max_entries, None)
}

/// Search the validated directory while retaining only the requested entry.
pub(crate) fn has_entry(data: &[u8], name: &str) -> bool {
    parse_directory(data, 1, Some(name)).is_ok_and(|archive| !archive.entries.is_empty())
}

/// List directory entries, optionally retaining only a matching decoded name.
fn parse_directory(data: &[u8], max_entries: usize, name_filter: Option<&str>) -> Result<ZipArchive, ZipError> {
    let r = Reader::new(data);
    let eocd = find_eocd(data).ok_or(ZipError::NotZip)?;
    let mut disk = u32::from(r.u16_le(eocd + 4).ok_or(ZipError::Truncated("end record"))?);
    let mut directory_disk = u32::from(r.u16_le(eocd + 6).ok_or(ZipError::Truncated("end record"))?);
    let mut disk_entries = u64::from(r.u16_le(eocd + 8).ok_or(ZipError::Truncated("end record"))?);
    if !matches!(disk, 0 | 0xffff) || !matches!(directory_disk, 0 | 0xffff) {
        return Err(ZipError::Unsupported("multi-volume ZIP archives".into()));
    }
    let mut total = u64::from(r.u16_le(eocd + 10).ok_or(ZipError::Truncated("end record"))?);
    let mut cd_size = u64::from(r.u32_le(eocd + 12).ok_or(ZipError::Truncated("end record"))?);
    let mut cd_offset = u64::from(r.u32_le(eocd + 16).ok_or(ZipError::Truncated("end record"))?);
    let mut zip64 = false;
    let mut warnings = Vec::new();
    let mut eocd_record_start = eocd as u64;
    if total == 0xffff || disk_entries == 0xffff || disk == 0xffff || directory_disk == 0xffff || cd_size == 0xffff_ffff || cd_offset == 0xffff_ffff {
        // ZIP64 locator sits right before the classic record.
        if let Some(loc) = eocd.checked_sub(20)
            && r.slice(loc, 4) == Some(b"PK\x06\x07")
        {
            if r.u32_le(loc + 4) != Some(0) || r.u32_le(loc + 16) != Some(1) {
                return Err(ZipError::Unsupported("multi-volume ZIP archives".into()));
            }
            let rec = r.u64_le(loc + 8).ok_or(ZipError::Truncated("zip64 locator"))?;
            let rec = usize::try_from(rec).ok().filter(|&rec| r.slice(rec, 4) == Some(b"PK\x06\x06"));
            let rec = rec.or_else(|| {
                let recovered = recover_zip64_record(data, loc, disk_entries, total, cd_size, cd_offset);
                if recovered.is_some() {
                    warnings.push("ZIP64 locator offset does not identify its end record; recovered the record before the locator".into());
                }
                recovered
            });
            if let Some(rec) = rec {
                let payload_size = r.u64_le(rec + 4).ok_or(ZipError::Truncated("zip64 record size"))?;
                if payload_size < 44 {
                    return Err(ZipError::Corrupt("ZIP64 end record is smaller than its fixed fields".into()));
                }
                let declared_end = (rec as u64)
                    .checked_add(12)
                    .and_then(|start| start.checked_add(payload_size))
                    .ok_or_else(|| ZipError::Corrupt("ZIP64 end record extent overflows".into()))?;
                // Fixed fields must be present before the locator. A bad declared extension
                // length cannot make the reader borrow locator or classic end-record bytes.
                if rec.checked_add(ZIP64_FIXED_SIZE).is_none_or(|end| end > loc) {
                    return Err(ZipError::Truncated("zip64 record fixed fields"));
                }
                if declared_end != loc as u64 {
                    warnings.push("ZIP64 end record's declared extent does not end at its locator; using bounded fixed fields".into());
                }
                disk = r.u32_le(rec + 16).ok_or(ZipError::Truncated("zip64 record"))?;
                directory_disk = r.u32_le(rec + 20).ok_or(ZipError::Truncated("zip64 record"))?;
                let zip64_disk_entries = r.u64_le(rec + 24).ok_or(ZipError::Truncated("zip64 record"))?;
                let zip64_total = r.u64_le(rec + 32).ok_or(ZipError::Truncated("zip64 record"))?;
                if (disk_entries != 0xffff && disk_entries != zip64_disk_entries) || (total != 0xffff && total != zip64_total) {
                    return Err(ZipError::Unsupported("multi-volume ZIP archives".into()));
                }
                disk_entries = zip64_disk_entries;
                total = zip64_total;
                let zip64_cd_size = r.u64_le(rec + 40).ok_or(ZipError::Truncated("zip64 record"))?;
                let zip64_cd_offset = r.u64_le(rec + 48).ok_or(ZipError::Truncated("zip64 record"))?;
                if (cd_size != 0xffff_ffff && cd_size != zip64_cd_size) || (cd_offset != 0xffff_ffff && cd_offset != zip64_cd_offset) {
                    return Err(ZipError::Corrupt("ZIP64 and classic directory bounds disagree".into()));
                }
                cd_size = zip64_cd_size;
                cd_offset = zip64_cd_offset;
                zip64 = true;
                eocd_record_start = rec as u64;
            } else {
                // A prepended stub can invalidate the locator's absolute offset. Keep the
                // classic-directory fallback, including its existing prefix adjustment.
                warnings.push("ZIP64 locator does not point to an end record; using the classic end record".into());
            }
        }
    }
    if disk != 0 || directory_disk != 0 || disk_entries != total {
        return Err(ZipError::Unsupported("multi-volume ZIP archives".into()));
    }
    // Self-extracting archives: the directory sits later than it claims by the stub length.
    let expected_end = cd_offset.checked_add(cd_size).ok_or_else(|| ZipError::Corrupt("directory bounds overflow".into()))?;
    let prefix = eocd_record_start.saturating_sub(expected_end);
    let start = usize::try_from(cd_offset.saturating_add(prefix)).map_err(|_| ZipError::Corrupt("directory offset".into()))?;
    let directory_end = (start as u64)
        .checked_add(cd_size)
        .filter(|&end| end <= eocd_record_start)
        .and_then(|end| usize::try_from(end).ok())
        .ok_or_else(|| ZipError::Corrupt("directory extends into its end record".into()))?;
    let mut entries = Vec::new();
    let mut pos = start;
    let mut truncated = false;
    let mut seen = 0u64;
    while seen < total {
        if entries.len() >= max_entries {
            truncated = true;
            break;
        }
        if pos.checked_add(46).is_none_or(|end| end > directory_end) {
            return Err(ZipError::Truncated("directory entry"));
        }
        if r.slice(pos, 4) != Some(b"PK\x01\x02") {
            if entries.is_empty() {
                return Err(ZipError::Corrupt(format!("no central directory header at offset {pos}")));
            }
            truncated = true;
            break;
        }
        let flags = r.u16_le(pos + 8).ok_or(ZipError::Truncated("directory entry"))?;
        let method = r.u16_le(pos + 10).ok_or(ZipError::Truncated("directory entry"))?;
        let crc32 = r.u32_le(pos + 16).ok_or(ZipError::Truncated("directory entry"))?;
        let mut csize = u64::from(r.u32_le(pos + 20).ok_or(ZipError::Truncated("directory entry"))?);
        let mut usize_ = u64::from(r.u32_le(pos + 24).ok_or(ZipError::Truncated("directory entry"))?);
        let name_len = usize::from(r.u16_le(pos + 28).ok_or(ZipError::Truncated("directory entry"))?);
        let extra_len = usize::from(r.u16_le(pos + 30).ok_or(ZipError::Truncated("directory entry"))?);
        let comment_len = usize::from(r.u16_le(pos + 32).ok_or(ZipError::Truncated("directory entry"))?);
        let entry_end = pos
            .checked_add(46 + name_len + extra_len + comment_len)
            .filter(|&end| end <= directory_end)
            .ok_or(ZipError::Truncated("directory entry fields"))?;
        let host = r.slice(pos + 5, 1).and_then(|b| b.first().copied()).unwrap_or(0);
        let attrs = r.u32_le(pos + 38).ok_or(ZipError::Truncated("entry attributes"))?;
        let unix_mode = matches!(host, 3 | 19).then_some(attrs >> 16);
        let mut local = u64::from(r.u32_le(pos + 42).ok_or(ZipError::Truncated("directory entry"))?);
        let mut entry_disk = u32::from(r.u16_le(pos + 34).ok_or(ZipError::Truncated("directory entry"))?);
        let name_bytes = r.slice(pos + 46, name_len).ok_or(ZipError::Truncated("entry name"))?;
        let extra = r.slice(pos + 46 + name_len, extra_len).ok_or(ZipError::Truncated("entry extra fields"))?;
        // Each extra field owns its payload: ZIP64 values cannot borrow a following field's bytes.
        let needs_zip64 = usize_ == 0xffff_ffff || csize == 0xffff_ffff || local == 0xffff_ffff || entry_disk == 0xffff;
        let mut found_zip64 = false;
        let mut unicode_name = None;
        let mut name_crc = None;
        let mut x = 0usize;
        let er = Reader::new(extra);
        // Trailing bytes too short for a field header are padding, not truncation.
        while x.saturating_add(4) <= extra.len() {
            let id = er.u16_le(x).ok_or(ZipError::Truncated("extra field header"))?;
            let len = usize::from(er.u16_le(x + 2).ok_or(ZipError::Truncated("extra field header"))?);
            let body = x + 4;
            let field = er.slice(body, len).ok_or(ZipError::Truncated("extra field payload"))?;
            if id == 0x0001 && !found_zip64 {
                let values = Reader::new(field);
                let mut f = 0;
                if usize_ == 0xffff_ffff {
                    usize_ = values.u64_le(f).ok_or(ZipError::Truncated("zip64 uncompressed size"))?;
                    f += 8;
                }
                if csize == 0xffff_ffff {
                    csize = values.u64_le(f).ok_or(ZipError::Truncated("zip64 compressed size"))?;
                    f += 8;
                }
                if local == 0xffff_ffff {
                    local = values.u64_le(f).ok_or(ZipError::Truncated("zip64 local header offset"))?;
                    f += 8;
                }
                if entry_disk == 0xffff {
                    entry_disk = values.u32_le(f).ok_or(ZipError::Truncated("zip64 entry disk"))?;
                }
                found_zip64 = true;
            } else if id == 0x7075 && flags & (1 << 11) == 0 && unicode_name.is_none() {
                let field = Reader::new(field);
                if field.u8(0) == Some(1) && field.u32_le(1) == Some(*name_crc.get_or_insert_with(|| self::crc32(name_bytes))) {
                    unicode_name = field.data().get(5..).and_then(|bytes| std::str::from_utf8(bytes).ok()).filter(|s| !s.is_empty());
                }
            }
            x = body + len;
        }
        if needs_zip64 && !found_zip64 {
            return Err(ZipError::Corrupt("missing ZIP64 extended information".into()));
        }
        if entry_disk != 0 {
            return Err(ZipError::Unsupported("multi-volume ZIP archives".into()));
        }
        let name = unicode_name.map(str::to_owned).unwrap_or_else(|| decode_name(name_bytes, flags)).replace('\\', "/");
        if name_filter.is_none_or(|wanted| wanted == name) {
            entries.push(ZipEntry {
                is_dir: name.ends_with('/') || unix_mode.is_some_and(|m| m & 0o170000 == 0o040000) || (matches!(host, 0 | 10 | 14) && attrs & 0x10 != 0),
                name,
                method,
                flags,
                crc32,
                compressed_size: csize,
                uncompressed_size: usize_,
                local_header_offset: local.saturating_add(prefix),
                encrypted: flags & 1 != 0,
                unix_mode,
            });
        }
        seen += 1;
        pos = entry_end;
    }
    Ok(ZipArchive { entries, declared_entries: total, truncated, zip64, prefix_bytes: prefix, warnings })
}

fn decode_name(bytes: &[u8], flags: u16) -> String {
    if flags & 0x0800 != 0 || bytes.is_ascii() {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    if let Ok(name) = std::str::from_utf8(bytes) {
        return name.to_owned();
    }
    const CP437: [char; 128] = [
        'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ', 'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', '¢', '£', '¥',
        '₧', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ', 'ª', 'º', '¿', '⌐', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', '╡', '╢', '╖', '╕', '╣', '║', '╗',
        '╝', '╜', '╛', '┐', '└', '┴', '┬', '├', '─', '┼', '╞', '╟', '╚', '╔', '╩', '╦', '╠', '═', '╬', '╧', '╨', '╤', '╥', '╙', '╘', '╒', '╓', '╫', '╪', '┘',
        '┌', '█', '▄', '▌', '▐', '▀', 'α', 'ß', 'Γ', 'π', 'Σ', 'σ', 'µ', 'τ', 'Φ', 'Θ', 'Ω', 'δ', '∞', 'φ', 'ε', '∩', '≡', '±', '≥', '≤', '⌠', '⌡', '÷', '≈',
        '°', '∙', '·', '√', 'ⁿ', '²', '■', '\u{00a0}',
    ];
    bytes.iter().map(|&byte| if byte.is_ascii() { char::from(byte) } else { CP437.get(usize::from(byte - 0x80)).copied().unwrap_or('\u{fffd}') }).collect()
}

pub(crate) fn find_eocd(data: &[u8]) -> Option<usize> {
    if data.len() < 22 {
        return None;
    }
    let from = data.len().saturating_sub(EOCD_SEARCH);
    let tail = data.get(from..)?;
    // Search backwards so a stray signature in the comment can't shadow the real record.
    let mut i = tail.len().checked_sub(22)?;
    let mut found = None;
    let mut last_signature = None;
    loop {
        if tail.get(i..i + 4) == Some(b"PK\x05\x06") {
            last_signature.get_or_insert(from + i);
            if let Some(comment_len) = tail.get(i + 20..i + 22).and_then(|bytes| <[u8; 2]>::try_from(bytes).ok()).map(u16::from_le_bytes)
                && i + 22 + usize::from(comment_len) == tail.len()
            {
                found = Some(from + i);
            }
        }
        if i == 0 {
            return found.or(last_signature);
        }
        i -= 1;
    }
}

/// The raw (possibly compressed) bytes of an entry.
pub fn raw_entry_data<'a>(data: &'a [u8], e: &ZipEntry) -> Result<&'a [u8], ZipError> {
    let r = Reader::new(data);
    let off = usize::try_from(e.local_header_offset).map_err(|_| ZipError::Corrupt("local header offset".into()))?;
    if r.slice(off, 4) != Some(b"PK\x03\x04") {
        return Err(ZipError::Corrupt(format!("no local header for {}", e.name)));
    }
    let n = usize::from(r.u16_le(off + 26).ok_or(ZipError::Truncated("local header"))?);
    let x = usize::from(r.u16_le(off + 28).ok_or(ZipError::Truncated("local header"))?);
    let start = off.checked_add(30 + n + x).ok_or(ZipError::Truncated("local header"))?;
    let len = usize::try_from(e.compressed_size).map_err(|_| ZipError::TooLarge(e.compressed_size))?;
    r.slice(start, len).ok_or(ZipError::Truncated("entry data"))
}

/// Read and decompress one entry, refusing to produce more than `max_bytes`.
pub fn read_entry(data: &[u8], e: &ZipEntry, max_bytes: u64) -> Result<Vec<u8>, ZipError> {
    let out = read_entry_unverified(data, e, max_bytes)?;
    let found = crc32(&out);
    if found != e.crc32 {
        return Err(ZipError::CrcMismatch { expected: e.crc32, found });
    }
    Ok(out)
}

/// Like [`read_entry`] but without comparing the stored CRC-32, so a caller that accepts a mismatch can still get the bytes.
pub fn read_entry_unverified(data: &[u8], e: &ZipEntry, max_bytes: u64) -> Result<Vec<u8>, ZipError> {
    if e.encrypted {
        return Err(ZipError::Unsupported(format!("{} is encrypted", e.name)));
    }
    if e.uncompressed_size > max_bytes {
        return Err(ZipError::TooLarge(max_bytes));
    }
    let raw = raw_entry_data(data, e)?;
    let cap = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    let out = match e.method {
        0 if raw.len() as u64 > max_bytes => return Err(ZipError::TooLarge(max_bytes)),
        0 => raw.to_vec(),
        8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, cap).map_err(|err| match err.status {
            miniz_oxide::inflate::TINFLStatus::HasMoreOutput => ZipError::TooLarge(max_bytes),
            _ => ZipError::Corrupt(format!("deflate stream for {} is damaged", e.name)),
        })?,
        12 | 95 => {
            let decoded = if e.method == 12 { crate::decompress::bzip2(raw, max_bytes) } else { crate::decompress::xz(raw, max_bytes) };
            decoded.map_err(|error| match error {
                crate::decompress::DecompressError::TooLarge => ZipError::TooLarge(max_bytes),
                _ => ZipError::Corrupt(format!("{} stream for {} is damaged: {error}", e.method_name(), e.name)),
            })?
        }
        14 => {
            let header = Reader::new(raw);
            let properties_size = header.u16_le(2).ok_or(ZipError::Truncated("LZMA properties header"))?;
            if properties_size != 5 {
                return Err(ZipError::Unsupported(format!("LZMA properties size {properties_size} for {}", e.name)));
            }
            let stream = raw.get(4..).ok_or(ZipError::Truncated("LZMA properties"))?;
            crate::decompress::lzma_zip(stream, e.uncompressed_size, e.flags & 2 != 0, max_bytes).map_err(|error| match error {
                crate::decompress::DecompressError::TooLarge => ZipError::TooLarge(max_bytes),
                _ => ZipError::Corrupt(format!("LZMA stream for {} is damaged: {error}", e.name)),
            })?
        }
        93 => crate::decompress::zstd(raw, max_bytes).map_err(|err| ZipError::Corrupt(format!("zstd stream for {} is damaged: {err}", e.name)))?,
        m => return Err(ZipError::Unsupported(format!("compression method {m} ({}) for {}", e.method_name(), e.name))),
    };
    if out.len() as u64 != e.uncompressed_size {
        return Err(ZipError::Corrupt(format!("decoded size for {} is {}, but its directory entry declares {}", e.name, out.len(), e.uncompressed_size)));
    }
    Ok(out)
}

/// CRC-32 (IEEE, as used by ZIP and PNG).
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal stored-only ZIP writer for tests.
    pub(crate) fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut cd = Vec::new();
        for (name, body) in files {
            let off = out.len() as u32;
            let crc = crc32(body);
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(body);
            cd.extend_from_slice(b"PK\x01\x02");
            cd.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            cd.extend_from_slice(&crc.to_le_bytes());
            cd.extend_from_slice(&(body.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(body.len() as u32).to_le_bytes());
            cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
            cd.extend_from_slice(&[0; 12]);
            cd.extend_from_slice(&off.to_le_bytes());
            cd.extend_from_slice(name.as_bytes());
        }
        let cd_off = out.len() as u32;
        out.extend_from_slice(&cd);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(cd.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_off.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn build_codec_entry(method: u16, compressed: &[u8], plain: &[u8]) -> Vec<u8> {
        let mut file = build(&[("entry", compressed)]);
        let central = 30 + "entry".len() + compressed.len();
        let version: u16 = if method == 12 { 46 } else { 63 };
        file[4..6].copy_from_slice(&version.to_le_bytes());
        file[8..10].copy_from_slice(&method.to_le_bytes());
        file[14..18].copy_from_slice(&crc32(plain).to_le_bytes());
        file[22..26].copy_from_slice(&(plain.len() as u32).to_le_bytes());
        file[central + 6..central + 8].copy_from_slice(&version.to_le_bytes());
        file[central + 10..central + 12].copy_from_slice(&method.to_le_bytes());
        file[central + 16..central + 20].copy_from_slice(&crc32(plain).to_le_bytes());
        file[central + 24..central + 28].copy_from_slice(&(plain.len() as u32).to_le_bytes());
        file
    }

    #[test]
    fn bzip2_and_xz_zip_entries_keep_size_crc_and_output_checks() {
        use craft_anything_testkit::firmware::{BZIP2_FIXTURE, BZIP2_PLAIN, XZ_FIXTURE, XZ_PLAIN};
        for (method, compressed, plain) in [(12, BZIP2_FIXTURE, BZIP2_PLAIN), (95, XZ_FIXTURE, XZ_PLAIN)] {
            let file = build_codec_entry(method, compressed, plain);
            let original = parse(&file, 1).unwrap().entries.remove(0);
            assert_eq!(read_entry(&file, &original, plain.len() as u64).unwrap(), plain);
            let mut entry = original.clone();
            entry.crc32 ^= 1;
            assert!(matches!(read_entry(&file, &entry, 100), Err(ZipError::CrcMismatch { .. })));
            assert_eq!(read_entry_unverified(&file, &entry, 100).unwrap(), plain);
            entry = original.clone();
            entry.uncompressed_size = 0;
            assert!(matches!(read_entry(&file, &entry, 100), Err(ZipError::Corrupt(_))));
            assert!(matches!(read_entry(&file, &entry, plain.len() as u64 - 1), Err(ZipError::TooLarge(_))));
            let mut damaged = file.clone();
            damaged[30 + "entry".len()] ^= 0xff;
            assert!(matches!(read_entry(&damaged, &original, 100), Err(ZipError::Corrupt(_))));
            entry = original;
            entry.encrypted = true;
            assert!(matches!(read_entry(&file, &entry, 100), Err(ZipError::Unsupported(_))));
        }
    }

    fn zip_lzma_member(plain: &[u8], end_marker: bool) -> Vec<u8> {
        let mut encoded = Vec::new();
        let size = if end_marker { None } else { Some(plain.len() as u64) };
        let options = lzma_rs::compress::Options { unpacked_size: lzma_rs::compress::UnpackedSize::WriteToHeader(size) };
        lzma_rs::lzma_compress_with_options(&mut std::io::Cursor::new(plain), &mut encoded, &options).unwrap();
        let mut compressed = vec![9, 4, 5, 0];
        compressed.extend_from_slice(&encoded[..5]);
        compressed.extend_from_slice(&encoded[13..]);
        let mut file = build(&[("entry", &compressed)]);
        let central = 30 + "entry".len() + compressed.len();
        let flags: u16 = if end_marker { 2 } else { 0 };
        for (version_at, flags_at, method_at, crc_at, size_at) in [(4, 6, 8, 14, 22), (central + 6, central + 8, central + 10, central + 16, central + 24)] {
            file[version_at..version_at + 2].copy_from_slice(&63u16.to_le_bytes());
            file[flags_at..flags_at + 2].copy_from_slice(&flags.to_le_bytes());
            file[method_at..method_at + 2].copy_from_slice(&14u16.to_le_bytes());
            file[crc_at..crc_at + 4].copy_from_slice(&crc32(plain).to_le_bytes());
            file[size_at..size_at + 4].copy_from_slice(&(plain.len() as u32).to_le_bytes());
        }
        file
    }

    #[test]
    fn zip_lzma_reads_size_delimited_and_end_marked_streams() {
        for end_marker in [false, true] {
            for plain in [&b"ZIP LZMA contents"[..], &[0; 23][..]] {
                let file = zip_lzma_member(plain, end_marker);
                let original = parse(&file, 1).unwrap().entries.remove(0);
                assert_eq!(read_entry(&file, &original, plain.len() as u64).unwrap(), plain);
                assert!(matches!(read_entry(&file, &original, plain.len() as u64 - 1), Err(ZipError::TooLarge(_))));
                let mut entry = original.clone();
                entry.crc32 ^= 1;
                assert!(matches!(read_entry(&file, &entry, 100), Err(ZipError::CrcMismatch { .. })));
                entry = original.clone();
                entry.uncompressed_size -= 1;
                assert!(read_entry(&file, &entry, 100).is_err());
                entry = original.clone();
                entry.compressed_size -= 1;
                assert!(read_entry(&file, &entry, 100).is_err());
                entry = original;
                entry.compressed_size += 1;
                assert!(read_entry(&file, &entry, 100).is_err());
            }
        }
    }

    #[test]
    fn zip_lzma_rejects_bad_properties_without_unbounded_dictionaries() {
        let mut file = zip_lzma_member(b"small", true);
        let entry = parse(&file, 1).unwrap().entries.remove(0);
        let body = 30 + "entry".len();
        file[body + 5..body + 9].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read_entry(&file, &entry, 5).unwrap(), b"small");
        file[body + 4] = 255;
        assert!(read_entry(&file, &entry, 5).is_err());
        file[body + 2..body + 4].copy_from_slice(&6u16.to_le_bytes());
        assert!(matches!(read_entry(&file, &entry, 5), Err(ZipError::Unsupported(_))));
    }

    fn build_named_entry(name: &[u8], flags: u16) -> Vec<u8> {
        let mut zip = build(&[(&"x".repeat(name.len()), b"")]);
        let central = 30 + name.len();
        zip[6..8].copy_from_slice(&flags.to_le_bytes());
        zip[30..central].copy_from_slice(name);
        zip[central + 8..central + 10].copy_from_slice(&flags.to_le_bytes());
        zip[central + 46..central + 46 + name.len()].copy_from_slice(name);
        zip
    }

    #[test]
    fn verified_unicode_path_overrides_a_legacy_zip_name() {
        let raw = b"notes.txt";
        let unicode = "笔记.txt";
        for (flags, version, checksum, expected) in
            [(0, 1, crc32(raw), unicode), (0, 1, crc32(b"old-name"), "notes.txt"), (0, 2, crc32(raw), "notes.txt"), (1 << 11, 1, crc32(raw), "notes.txt")]
        {
            let mut zip = build_named_entry(raw, flags);
            let central = 30 + raw.len();
            let end = central + 46 + raw.len();
            let mut extra = 0x7075_u16.to_le_bytes().to_vec();
            extra.extend_from_slice(&((5 + unicode.len()) as u16).to_le_bytes());
            extra.push(version);
            extra.extend_from_slice(&checksum.to_le_bytes());
            extra.extend_from_slice(unicode.as_bytes());
            zip[central + 30..central + 32].copy_from_slice(&(extra.len() as u16).to_le_bytes());
            zip[end + 12..end + 16].copy_from_slice(&((end - central + extra.len()) as u32).to_le_bytes());
            zip.splice(end..end, extra);
            assert_eq!(parse(&zip, 1).unwrap().entries[0].name, expected);
        }
    }

    #[test]
    fn directory_markers_do_not_require_a_trailing_slash() {
        let sample = build(&[("folder", b"")]);
        let central = 30 + "folder".len();
        for (host, attrs, directory, special) in [
            (0, 0x10u32, true, None),
            (10, 0x10, true, None),
            (3, 0o040755 << 16, true, None),
            (3, 0o120777 << 16, false, Some("symlink")),
            (0, 0x20, false, None),
            (14, 0x10, true, None),
            (14, 0x20, false, None),
            (19, 0o040755 << 16, true, None),
            (19, 0o100644 << 16, false, None),
            (19, 0o120777 << 16, false, Some("symlink")),
            (42, 0x10, false, None),
        ] {
            let mut file = sample.clone();
            file[central + 5] = host;
            file[central + 38..central + 42].copy_from_slice(&attrs.to_le_bytes());
            let archive = parse(&file, 1).unwrap();
            let entry = &archive.entries[0];
            assert_eq!((entry.is_dir, entry.special_kind()), (directory, special), "host {host}, attributes {attrs:#x}");
        }
    }

    #[test]
    fn a_cp437_name_without_the_utf8_flag_is_decoded() {
        let zip = build_named_entry(b"caf\x82.txt", 0);
        assert_eq!(parse(&zip, 10).unwrap().entries[0].name, "café.txt");
    }

    #[test]
    fn a_cp437_box_drawing_name_without_the_utf8_flag_is_decoded() {
        let zip = build_named_entry(b"\xc9.txt", 0);
        assert_eq!(parse(&zip, 10).unwrap().entries[0].name, "╔.txt");
    }

    #[test]
    fn a_utf8_name_with_the_utf8_flag_is_preserved() {
        let zip = build_named_entry("한글/é.txt".as_bytes(), 0x0800);
        assert_eq!(parse(&zip, 10).unwrap().entries[0].name, "한글/é.txt");
    }

    #[test]
    fn a_valid_utf8_name_without_the_utf8_flag_is_preserved() {
        let zip = build_named_entry("é.txt".as_bytes(), 0);
        assert_eq!(parse(&zip, 10).unwrap().entries[0].name, "é.txt");
    }

    #[test]
    fn an_ascii_name_is_preserved() {
        let zip = build_named_entry(b"dir/file.txt", 0);
        assert_eq!(parse(&zip, 10).unwrap().entries[0].name, "dir/file.txt");
    }

    #[test]
    fn valid_entry_survives_eocd_signature_in_comment() {
        let mut zip = build(&[("a.txt", b"hello")]);
        let comment = b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
        let eocd = zip.len() - 22;
        zip[eocd + 20..eocd + 22].copy_from_slice(&(comment.len() as u16).to_le_bytes());
        zip.extend_from_slice(comment);

        assert_eq!(parse(&zip, 10).unwrap().entries.len(), 1);
    }

    #[test]
    fn valid_entry_survives_bytes_after_eocd_comment() {
        let mut zip = build(&[("a.txt", b"hello")]);
        zip.extend_from_slice(b"tail");

        assert_eq!(parse(&zip, 10).unwrap().entries.len(), 1);
    }

    #[test]
    fn checks_the_zip64_record_extent_including_extension_bytes() {
        let mut record = vec![0; 56];
        record[..4].copy_from_slice(b"PK\x06\x06");
        record[4..12].copy_from_slice(&46u64.to_le_bytes());
        record.extend([1, 2]); // extensible data
        record.extend(b"PK\x06\x07");
        record.extend([0; 12]);
        record.extend(1u32.to_le_bytes());
        record.extend(b"PK\x05\x06");
        record.extend([0; 4]);
        record.extend([0xff; 4]);
        record.extend([0; 10]);
        assert!(parse(&record, 1).unwrap().zip64);
        for len in [44u64, 47] {
            record[4..12].copy_from_slice(&len.to_le_bytes());
            let archive = parse(&record, 1).unwrap();
            assert!(archive.zip64);
            assert_eq!(archive.warnings.len(), 1);
        }
        for len in [43u64, u64::MAX] {
            record[4..12].copy_from_slice(&len.to_le_bytes());
            assert!(parse(&record, 1).is_err(), "{len}");
        }
    }

    #[test]
    fn lists_and_reads_stored_entries() {
        let z = build(&[("a.txt", b"hello"), ("dir/b.js", b"console.log(1)")]);
        let a = parse(&z, 100).unwrap();
        assert_eq!(a.entries.len(), 2);
        assert_eq!(a.entries[1].name, "dir/b.js");
        assert_eq!(read_entry(&z, &a.entries[0], 100).unwrap(), b"hello");
        assert!(matches!(read_entry(&z, &a.entries[1], 3), Err(ZipError::TooLarge(3))));
    }

    #[test]
    fn zstandard_members_are_read_with_their_limits_and_checks() {
        // A raw Zstandard frame holding "hello zip" (single segment, one raw block).
        let body = b"hello zip";
        let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, body.len() as u8];
        let block_header = ((body.len() as u32) << 3) | 1;
        frame.extend(&block_header.to_le_bytes()[..3]);
        frame.extend(body);
        let mut file = vec![0u8; 30];
        file[..4].copy_from_slice(b"PK\x03\x04");
        file.extend(&frame);
        let mut e = ZipEntry {
            name: "x".into(),
            method: 93,
            flags: 0,
            crc32: crc32(body),
            compressed_size: frame.len() as u64,
            uncompressed_size: body.len() as u64,
            local_header_offset: 0,
            is_dir: false,
            encrypted: false,
            unix_mode: None,
        };
        assert_eq!(read_entry(&file, &e, 100).unwrap(), body);
        assert!(matches!(read_entry(&file, &e, 3), Err(ZipError::TooLarge(3))));
        e.crc32 ^= 1;
        assert!(matches!(read_entry(&file, &e, 100), Err(ZipError::CrcMismatch { .. })));
    }

    #[test]
    fn deflate_round_trip_and_crc_check() {
        let body = b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".repeat(20);
        let comp = miniz_oxide::deflate::compress_to_vec(&body, 6);
        let e = ZipEntry {
            name: "x".into(),
            method: 8,
            flags: 0,
            crc32: crc32(&body),
            compressed_size: comp.len() as u64,
            uncompressed_size: body.len() as u64,
            local_header_offset: 0,
            is_dir: false,
            encrypted: false,
            unix_mode: None,
        };
        let mut z = b"PK\x03\x04".to_vec();
        z.extend_from_slice(&[0; 26]);
        z.extend_from_slice(&comp);
        assert_eq!(read_entry(&z, &e, 1 << 20).unwrap(), body);
        // A lying uncompressed size can't make us exceed the cap.
        let liar = ZipEntry { uncompressed_size: 10, ..e.clone() };
        assert!(matches!(read_entry(&z, &liar, 10), Err(ZipError::TooLarge(10))));
        let bad = ZipEntry { crc32: 1, ..e };
        assert!(matches!(read_entry(&z, &bad, 1 << 20), Err(ZipError::CrcMismatch { .. })));
    }

    #[test]
    fn hostile_inputs_do_not_panic() {
        let z = build(&[("a", b"1"), ("b", b"22")]);
        for cut in 0..z.len() {
            if let Ok(a) = parse(&z[..cut], 10) {
                for e in &a.entries {
                    let _ = read_entry(&z[..cut], e, 100);
                }
            }
        }
        for i in 0..z.len() {
            let mut m = z.clone();
            m[i] ^= 0xff;
            if let Ok(a) = parse(&m, 10) {
                for e in &a.entries {
                    let _ = read_entry(&m, e, 100);
                }
            }
        }
        assert_eq!(parse(b"", 1), Err(ZipError::NotZip));
    }

    #[test]
    fn crc_known_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }
}
