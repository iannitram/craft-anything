// Test-only compatibility oracle retained from elf.rs at acacd13b.
// Original implementation: Brandon Thomas; overflow hardening: GGWA.
// MIT OR Apache-2.0, unchanged parser behavior; shared production code lives in craft-elf.
use crate::{FormatError, hex_addr};
use serde::Serialize;
const MAX_PHDRS: usize = 4096;
const MAX_DYN: usize = 65_536;

// Reuse the unchanged app byte reader; the shared parser uses its own independent reader.
use super::E;

/// One program header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProgramHeader {
    pub kind: String,
    pub flags: String,
    pub offset: u64,
    #[serde(with = "hex_addr")]
    pub vaddr: u64,
    pub file_size: u64,
    pub mem_size: u64,
    pub align: u64,
    #[serde(skip)]
    pub p_type: u32,
    #[serde(skip)]
    pub p_flags: u32,
}

fn phdr_name(t: u32) -> String {
    match t {
        0 => "NULL".into(),
        1 => "LOAD".into(),
        2 => "DYNAMIC".into(),
        3 => "INTERP".into(),
        4 => "NOTE".into(),
        5 => "SHLIB".into(),
        6 => "PHDR".into(),
        7 => "TLS".into(),
        0x6474_e550 => "GNU_EH_FRAME".into(),
        0x6474_e551 => "GNU_STACK".into(),
        0x6474_e552 => "GNU_RELRO".into(),
        0x6474_e553 => "GNU_PROPERTY".into(),
        0x7000_0001 => "ARM_EXIDX".into(),
        other => format!("{other:#x}"),
    }
}

fn perm_string(f: u32) -> String {
    format!("{}{}{}", if f & 4 != 0 { 'r' } else { '-' }, if f & 2 != 0 { 'w' } else { '-' }, if f & 1 != 0 { 'x' } else { '-' })
}

/// The program headers of an ELF file.
pub fn program_headers(data: &[u8]) -> Result<Vec<ProgramHeader>, FormatError> {
    let e = E::new(data)?;
    let (phoff, phentsize, phnum) = if e.is64 { (e.u64(32), e.u16(54), e.u16(56)) } else { (e.u32(28).map(u64::from), e.u16(42), e.u16(44)) };
    let phoff = usize::try_from(phoff.ok_or_else(|| FormatError::Truncated("ELF header".into()))?).map_err(|_| FormatError::Malformed("phoff".into()))?;
    let phentsize = usize::from(phentsize.unwrap_or(0));
    let phnum = usize::from(phnum.unwrap_or(0)).min(MAX_PHDRS);
    let min = if e.is64 { 56 } else { 32 };
    if phnum > 0 && phentsize < min {
        return Err(FormatError::Malformed("program header entry too small".into()));
    }
    let mut out = Vec::new();
    for i in 0..phnum {
        // The fields below sit within `phentsize` bytes of `o`, so their offsets can't overflow either.
        let Some(o) = i.checked_mul(phentsize).and_then(|x| x.checked_add(phoff)).filter(|&o| o.checked_add(phentsize).is_some()) else { break };
        let ph = if e.is64 {
            (e.u32(o), e.u32(o + 4), e.u64(o + 8), e.u64(o + 16), e.u64(o + 32), e.u64(o + 40), e.u64(o + 48))
        } else {
            (
                e.u32(o),
                e.u32(o + 24),
                e.u32(o + 4).map(u64::from),
                e.u32(o + 8).map(u64::from),
                e.u32(o + 16).map(u64::from),
                e.u32(o + 20).map(u64::from),
                e.u32(o + 28).map(u64::from),
            )
        };
        let (Some(t), Some(f), Some(off), Some(va), Some(fs), Some(ms), Some(al)) = ph else { break };
        out.push(ProgramHeader {
            kind: phdr_name(t),
            flags: perm_string(f),
            offset: off,
            vaddr: va,
            file_size: fs,
            mem_size: ms,
            align: al,
            p_type: t,
            p_flags: f,
        });
    }
    Ok(out)
}

/// The (tag, value) pairs of the dynamic section, up to its `DT_NULL`.
fn dynamic_entries(e: &E<'_>, phdrs: &[ProgramHeader]) -> Vec<(u64, u64)> {
    let mut out = Vec::new();
    let Some(d) = phdrs.iter().find(|p| p.p_type == 2) else { return out };
    let ent = 2 * e.word_size();
    let n = usize::try_from(d.file_size).unwrap_or(0) / ent;
    let start = usize::try_from(d.offset).unwrap_or(usize::MAX);
    for i in 0..n.min(MAX_DYN) {
        let Some(o) = i.checked_mul(ent).and_then(|x| start.checked_add(x)) else { break };
        let (Some(tag), Some(val)) = (e.word(o), o.checked_add(e.word_size()).and_then(|v| e.word(v))) else { break };
        if tag == 0 {
            break;
        }
        out.push((tag, val));
    }
    out
}

pub(super) fn dynamic(data: &[u8], headers: &[ProgramHeader]) -> Vec<(u64, u64)> {
    E::new(data).map(|e| dynamic_entries(&e, headers)).unwrap_or_default()
}
