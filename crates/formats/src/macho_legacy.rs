// Test-only original primitive implementation from db8b9777.
// Brandon Thomas, GGWA, BFlat and Afloat; MIT OR Apache-2.0.
use crate::{FormatError, bytes::Reader};
use serde::Serialize;
const MAX_CMDS: usize = 4096;
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FatSlice {
    pub arch: String,
    pub cputype: u32,
    pub cpusubtype: u32,
    pub offset: u64,
    pub size: u64,
    pub align: u32,
}

pub fn cpu_name(cputype: u32, sub: u32) -> String {
    match cputype {
        7 => "i386".into(),
        0x0100_0007 if sub & 0x00ff_ffff == 8 => "x86_64h".into(),
        0x0100_0007 => "x86_64".into(),
        12 => "arm".into(),
        0x0100_000c => {
            if sub & 0xff == 2 {
                "arm64e".into()
            } else {
                "arm64".into()
            }
        }
        0x0200_000c => "arm64_32".into(),
        18 => "ppc".into(),
        0x0100_0012 => "ppc64".into(),
        other => format!("cpu{other:#x}"),
    }
}

/// The slices of a universal (fat) binary.
pub fn fat_slices(data: &[u8]) -> Result<Vec<FatSlice>, FormatError> {
    let r = Reader::new(data);
    let magic = r.u32_be(0).ok_or_else(|| FormatError::Truncated("fat header".into()))?;
    let wide = match magic {
        0xcafe_babe => false,
        0xcafe_babf => true,
        _ => return Err(FormatError::Malformed("not a universal binary".into())),
    };
    let n = r.u32_be(4).ok_or_else(|| FormatError::Truncated("fat header".into()))? as usize;
    if n == 0 || n > 64 {
        return Err(FormatError::Malformed(format!("{n} slices (Java class files share this magic)")));
    }
    let mut out = Vec::new();
    for i in 0..n {
        let s = if wide {
            let o = 8 + i * 32;
            (r.u32_be(o), r.u32_be(o + 4), r.u64_be(o + 8), r.u64_be(o + 16), r.u32_be(o + 24))
        } else {
            let o = 8 + i * 20;
            (r.u32_be(o), r.u32_be(o + 4), r.u32_be(o + 8).map(u64::from), r.u32_be(o + 12).map(u64::from), r.u32_be(o + 16))
        };
        let (Some(cpu), Some(sub), Some(offset), Some(size), Some(align)) = s else {
            return Err(FormatError::Truncated("fat arch table".into()));
        };
        if offset.checked_add(size).is_none_or(|end| end > data.len() as u64) {
            return Err(FormatError::Truncated(format!("slice {i} runs past the end of the file")));
        }
        out.push(FatSlice { arch: cpu_name(cpu, sub), cputype: cpu, cpusubtype: sub, offset, size, align });
    }
    Ok(out)
}

pub fn load_command_name(cmd: u32) -> String {
    match cmd {
        0x1 => "LC_SEGMENT",
        0x2 => "LC_SYMTAB",
        0x5 => "LC_UNIXTHREAD",
        0xb => "LC_DYSYMTAB",
        0xc => "LC_LOAD_DYLIB",
        0xd => "LC_ID_DYLIB",
        0xe => "LC_LOAD_DYLINKER",
        0x19 => "LC_SEGMENT_64",
        0x1b => "LC_UUID",
        0x1d => "LC_CODE_SIGNATURE",
        0x1e => "LC_SEGMENT_SPLIT_INFO",
        0x20 => "LC_LAZY_LOAD_DYLIB",
        0x21 => "LC_ENCRYPTION_INFO",
        0x22 => "LC_DYLD_INFO",
        0x8000_0022 => "LC_DYLD_INFO_ONLY",
        0x24 => "LC_VERSION_MIN_MACOSX",
        0x25 => "LC_VERSION_MIN_IPHONEOS",
        0x26 => "LC_FUNCTION_STARTS",
        0x29 => "LC_DATA_IN_CODE",
        0x2a => "LC_SOURCE_VERSION",
        0x2c => "LC_ENCRYPTION_INFO_64",
        0x32 => "LC_BUILD_VERSION",
        0x8000_0018 => "LC_LOAD_WEAK_DYLIB",
        0x8000_001c => "LC_RPATH",
        0x8000_001f => "LC_REEXPORT_DYLIB",
        0x8000_0028 => "LC_MAIN",
        0x8000_0033 => "LC_DYLD_EXPORTS_TRIE",
        0x8000_0034 => "LC_DYLD_CHAINED_FIXUPS",
        other => return format!("{other:#x}"),
    }
    .into()
}

/// Fixed bytes required before decoding a known load command's fields.
pub(crate) fn load_command_min_size(cmd: u32) -> usize {
    match cmd {
        0x1 => 56,
        0x2 | 0x1b | 0x2c | 0x32 | 0x8000_0028 => 24,
        0x5 => 16,
        0xb => 80,
        0xc | 0xd | 0x20 | 0x8000_0018 | 0x8000_001f | 0x8000_0023 => 24,
        0xe | 0xf | 0x12 | 0x13 | 0x14 | 0x15 | 0x27 | 0x8000_001c => 12,
        0x19 => 72,
        0x1d | 0x1e | 0x24 | 0x25 | 0x26 | 0x29 | 0x2a | 0x2b | 0x2e | 0x2f | 0x30 | 0x8000_0033 | 0x8000_0034 => 16,
        0x21 => 20,
        0x22 | 0x8000_0022 => 48,
        _ => 8,
    }
}

pub(super) fn nibble_version(v: u32) -> String {
    format!("{}.{}.{}", v >> 16, (v >> 8) & 0xff, v & 0xff)
}

// A thread command can carry several flavor/count/state records. Counts are 32-bit words.
pub(super) fn unixthread_pc(data: &[u8], cpu: u32, is64: bool) -> Option<u64> {
    let r = Reader::new(data);
    let mut pos = 0usize;
    for _ in 0..MAX_CMDS {
        let flavor = r.u32_le(pos)?;
        let count = r.u32_le(pos.checked_add(4)?)?;
        let state = pos.checked_add(8)?;
        let size = usize::try_from(count).ok()?.checked_mul(4)?;
        let record = Reader::new(r.slice(state, size)?);
        let pc = match (cpu, is64, flavor) {
            (0x0100_0007, true, 4) if count >= 42 => record.u64_le(16 * 8),
            (0x0100_000c, true, 6) if count >= 68 => record.u64_le(32 * 8),
            (7, false, 1) if count >= 16 => record.u32_le(10 * 4).map(u64::from),
            _ => None,
        };
        if pc.is_some() {
            return pc;
        }
        pos = state.checked_add(size)?;
    }
    None
}
