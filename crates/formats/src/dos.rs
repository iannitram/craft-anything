//! DOS programs: MZ executables (header, relocations, entry, load module) and .COM images.

use crate::FormatError;
use crate::binary::{Arch, Binary, Format};
use crate::bytes::Reader;
use serde::Serialize;
use serde_json::json;

/// Real-mode load segment we pretend the loader picked, so linear addresses are stable.
pub const MZ_LOAD_SEGMENT: u16 = 0;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MzRelocation {
    pub offset: u16,
    pub segment: u16,
    /// Linear offset in the load module that the loader patches.
    pub linear: u32,
    /// True when the patched word lies inside the load module.
    pub in_module: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MzImage {
    pub header_bytes: u32,
    /// File bytes the loader copies (header excluded), as the header declares.
    pub load_module_offset: u32,
    pub load_module_size: u32,
    /// Bytes after the declared image (overlays, appended data).
    pub overlay_size: u64,
    pub entry_cs: u16,
    pub entry_ip: u16,
    /// Linear entry offset inside the load module.
    pub entry_linear: u32,
    pub stack_ss: u16,
    pub stack_sp: u16,
    pub min_alloc_paragraphs: u16,
    pub max_alloc_paragraphs: u16,
    pub checksum: u16,
    pub relocations: Vec<MzRelocation>,
    /// Plain-English problems (entry outside the module, relocations past the end…).
    pub problems: Vec<String>,
    /// e_lfanew points at a newer header (NE/LE/PE) this tool does not load as DOS.
    pub extended_header: Option<String>,
}

/// Parse a DOS MZ header and relocation table.
pub fn parse_mz(data: &[u8]) -> Result<MzImage, FormatError> {
    let r = Reader::new(data);
    if !data.starts_with(b"MZ") && !data.starts_with(b"ZM") {
        return Err(FormatError::Malformed("no MZ signature".into()));
    }
    let t = |v: Option<u16>| v.ok_or_else(|| FormatError::Truncated("MZ header".into()));
    let last_page = t(r.u16_le(2))?;
    let pages = t(r.u16_le(4))?;
    let nrelocs = t(r.u16_le(6))?;
    let header_paras = t(r.u16_le(8))?;
    let min_alloc = t(r.u16_le(10))?;
    let max_alloc = t(r.u16_le(12))?;
    let ss = t(r.u16_le(14))?;
    let sp = t(r.u16_le(16))?;
    let checksum = t(r.u16_le(18))?;
    let ip = t(r.u16_le(20))?;
    let cs = t(r.u16_le(22))?;
    let lfarlc = t(r.u16_le(24))?;
    let header_bytes = u32::from(header_paras) * 16;
    let image_size = if pages == 0 {
        0
    } else {
        let full = u32::from(pages) * 512;
        if last_page == 0 { full } else { full - 512 + u32::from(last_page.min(512)) }
    };
    let mut problems = Vec::new();
    if pages == 0 {
        problems.push("the header declares zero 512-byte pages".to_string());
    }
    if last_page > 512 {
        problems.push(format!("the last page declares {last_page} bytes, more than 512"));
    }
    if header_bytes < 28 {
        problems.push(format!("the {header_bytes}-byte header is shorter than the 28-byte MZ header"));
    }
    if pages != 0 && header_bytes > image_size {
        problems.push(format!("the {header_bytes}-byte header is larger than the {image_size}-byte image"));
    }
    let file_len = data.len() as u64;
    if u64::from(image_size) > file_len {
        problems.push(format!("the header declares {image_size} bytes but the file has {file_len}"));
    }
    let load_module_size = image_size.saturating_sub(header_bytes).min(u32::try_from(file_len.saturating_sub(u64::from(header_bytes))).unwrap_or(u32::MAX));
    let entry_linear = (u32::from(cs) << 4).wrapping_add(u32::from(ip)) & 0xf_ffff;
    if entry_linear >= load_module_size {
        problems.push(format!("entry {cs:04x}:{ip:04x} is outside the {load_module_size}-byte load module"));
    }
    let mut relocations = Vec::new();
    let header = Reader::new(r.slice(0, (header_bytes as usize).min(data.len())).unwrap_or(&[]));
    for i in 0..usize::from(nrelocs) {
        let o = usize::from(lfarlc) + i * 4;
        let (Some(off), Some(seg)) = (header.u16_le(o), header.u16_le(o + 2)) else {
            problems.push(format!("relocation table cut short after {i} of {nrelocs} entries"));
            break;
        };
        let linear = (u32::from(seg) << 4) + u32::from(off);
        let in_module = linear.saturating_add(2) <= load_module_size;
        if !in_module {
            problems.push(format!("relocation {seg:04x}:{off:04x} points outside the load module"));
        }
        relocations.push(MzRelocation { offset: off, segment: seg, linear, in_module });
    }
    let lfanew = r.u32_le(0x3c).unwrap_or(0) as usize;
    let extended_header = if lfarlc >= 0x40 {
        r.slice(lfanew, 2).and_then(|sig| match sig {
            b"NE" => Some("NE (16-bit Windows)".to_string()),
            b"LE" | b"LX" => Some("LE/LX (OS/2 or VxD)".to_string()),
            b"PE" if crate::pe::has_pe_header(data) => Some("PE (32/64-bit Windows)".to_string()),
            _ => None,
        })
    } else {
        None
    };
    Ok(MzImage {
        header_bytes,
        load_module_offset: header_bytes,
        load_module_size,
        overlay_size: file_len.saturating_sub(u64::from(image_size.max(header_bytes))),
        entry_cs: cs,
        entry_ip: ip,
        entry_linear,
        stack_ss: ss,
        stack_sp: sp,
        min_alloc_paragraphs: min_alloc,
        max_alloc_paragraphs: max_alloc,
        checksum,
        relocations,
        problems,
        extended_header,
    })
}

/// A Binary for an MZ program: one 16-bit code section at linear address 0 (load module).
pub fn mz_binary(data: &[u8]) -> Result<Binary, FormatError> {
    let mz = parse_mz(data)?;
    let mut b = Binary::raw(Arch::X86_16, 0, u64::from(mz.load_module_size), u64::from(mz.load_module_offset));
    b.format = Format::DosMz;
    b.entry = Some(u64::from(mz.entry_linear));
    if let Some(s) = b.sections.first_mut() {
        s.name = "load_module".into();
    }
    b.details = json!({"mz": mz});
    b.warnings.extend(mz.problems.iter().cloned());
    Ok(b)
}

/// A Binary for a .COM program: loaded at offset 0x100 of its segment.
pub fn com_binary(data: &[u8]) -> Result<Binary, FormatError> {
    if data.len() > 0xff00 {
        return Err(FormatError::TooLarge(format!("{} bytes; a .COM program fits in 65280", data.len())));
    }
    let mut b = Binary::raw(Arch::X86_16, 0x100, data.len() as u64, 0);
    b.format = Format::DosCom;
    if let Some(s) = b.sections.first_mut() {
        s.name = "com_image".into();
    }
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Contradictory size fields are reported as problems while the rest of the header still reads.
    #[test]
    fn contradictory_mz_size_declarations_are_reported() {
        let base = craft_anything_testkit::dos_mz();
        let cases = [
            (2, 513u16, "the last page declares 513 bytes, more than 512"),
            (4, 0, "the header declares zero 512-byte pages"),
            (8, 1, "the 16-byte header is shorter than the 28-byte MZ header"),
            (8, 100, "the 1600-byte header is larger than the 79-byte image"),
        ];
        for (offset, value, problem) in cases {
            let mut data = base.clone();
            data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
            let image = parse_mz(&data).unwrap();
            assert!(image.problems.iter().any(|p| p == problem), "offset {offset}, value {value}: {:?}", image.problems);
            let binary = crate::parse_binary(&data).unwrap();
            assert_eq!(binary.format, Format::DosMz);
            assert!(binary.warnings.iter().any(|w| w == problem));
        }
        let last_page = {
            let mut data = base.clone();
            data[2..4].copy_from_slice(&513u16.to_le_bytes());
            parse_mz(&data).unwrap()
        };
        assert_eq!((last_page.entry_cs, last_page.entry_ip), (0, 0));
        assert_eq!(last_page.relocations.len(), 2);
        assert_eq!(last_page.load_module_size, 15);
        assert!(parse_mz(&base).unwrap().problems.is_empty());
        for last_page in [0u16, 512] {
            let mut full_page = base.clone();
            full_page.resize(512, 0);
            full_page[2..4].copy_from_slice(&last_page.to_le_bytes());
            let image = parse_mz(&full_page).unwrap();
            assert_eq!(image.load_module_size, 448);
            assert!(image.problems.is_empty());
        }
        let truncated = parse_mz(&base[..base.len() - 1]).unwrap();
        assert!(!truncated.problems.is_empty());
        assert_eq!(truncated.load_module_offset, 64);
        assert_eq!(truncated.load_module_size, 14);
        assert_eq!(truncated.relocations.len(), 2);
    }

    #[test]
    fn mz_header_and_relocations() {
        let d = craft_anything_testkit::dos_mz();
        let mz = parse_mz(&d).unwrap();
        assert_eq!(mz.relocations.len(), 2);
        assert_eq!(mz.header_bytes, 64);
        assert_eq!(mz.load_module_size, 15);
        assert!(mz.problems.is_empty(), "{:?}", mz.problems);
        let b = crate::parse_binary(&d).unwrap();
        assert_eq!(b.format, Format::DosMz);
        assert_eq!(b.read(&d, 0, 2), Some(&[0xb4u8, 0x09][..]));
    }

    #[test]
    fn com_image() {
        let d = craft_anything_testkit::dos_com();
        let b = com_binary(&d).unwrap();
        assert_eq!(b.entry, Some(0x100));
        assert_eq!(b.read(&d, 0x10c, 3), Some(&b"Hi$"[..]));
    }

    #[test]
    fn damaged_mz_never_panics() {
        for m in craft_anything_testkit::mutations(&craft_anything_testkit::dos_mz()) {
            let _ = parse_mz(&m);
            let _ = crate::parse_binary(&m);
        }
    }
}
