//! Mach-O details: universal slices, load commands, stubs and pointer slots from the indirect
//! symbol table, function starts, mitigations, and Objective-C metadata (classes, methods,
//! categories, protocols).

use crate::binary::{Binary, ImportSlot};
use crate::bytes::Reader;
use crate::{FormatError, hex_addr};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeSet;

const MAX_CMDS: usize = 4096;
const MAX_INDIRECT: usize = 500_000;
const MAX_OBJC: usize = 50_000;
const MAX_METHODS: usize = 4096;

pub(crate) use craft_macho::load_command_min_size;
pub use craft_macho::{FatSlice, cpu_name, load_command_name};
use craft_macho::{nibble_version, unixthread_pc};

/// The slices of a universal binary, preserving the app error contract.
pub fn fat_slices(data: &[u8]) -> Result<Vec<FatSlice>, FormatError> {
    craft_macho::fat_slices(data).map_err(|error| match error {
        craft_macho::MachoError::Truncated(context) => FormatError::Truncated(context),
        craft_macho::MachoError::Malformed(context) => FormatError::Malformed(context),
    })
}

#[cfg(test)]
#[path = "macho_legacy.rs"]
mod legacy;

#[derive(Debug, Clone)]
struct MSection {
    sectname: String,
    addr: u64,
    size: u64,
    flags: u32,
    reserved1: u32,
    reserved2: u32,
}

#[derive(Debug, Clone)]
struct MSegment {
    name: String,
    vmaddr: u64,
    fileoff: u64,
    filesize: u64,
}

/// Add Mach-O facts. `data` is the (thin) slice.
pub fn enrich(data: &[u8], b: &mut Binary) {
    let r = Reader::new(data);
    let (is64, ok) = match r.u32_le(0) {
        Some(0xfeed_facf) => (true, true),
        Some(0xfeed_face) => (false, true),
        _ => (false, false),
    };
    if !ok {
        b.warnings.push("big-endian Mach-O: load commands not decoded".into());
        return;
    }
    let ncmds = r.u32_le(16).unwrap_or(0) as usize;
    let sizeofcmds = r.u32_le(20).unwrap_or(0) as usize;
    let header_flags = r.u32_le(24).unwrap_or(0);
    let mut pos: usize = if is64 { 32 } else { 28 };
    let limit = pos.saturating_add(sizeofcmds).min(data.len());
    if sizeofcmds > data.len().saturating_sub(pos) {
        b.warnings.push("sizeofcmds runs past the end of the file".into());
    }
    let mut sections = Vec::new();
    let mut segments = Vec::new();
    let mut symtab = None;
    let mut indirect = None;
    let mut entryoff = None;
    let mut thread_pc = None;
    let mut fstarts = None;
    let mut cmds_json = Vec::new();
    // Preserve each dependency's ordinal even when its load command has no usable path.
    let mut dylib_ordinals = Vec::new();
    for _ in 0..ncmds.min(MAX_CMDS) {
        if pos.checked_add(8).is_none_or(|end| end > limit) {
            b.warnings.push("load command header runs past sizeofcmds; stopped".into());
            break;
        }
        let (Some(cmd), Some(size)) = (r.u32_le(pos), r.u32_le(pos + 4)) else {
            b.warnings.push("load commands cut short".into());
            break;
        };
        let size = size as usize;
        if size < 8 {
            b.warnings.push("a load command has a size below 8; stopped".into());
            break;
        }
        let Some(end) = pos.checked_add(size).filter(|&end| end <= limit) else {
            b.warnings.push("a load command runs past sizeofcmds; stopped".into());
            break;
        };
        if cmds_json.len() < 512 {
            cmds_json.push(json!({"cmd": load_command_name(cmd), "size": size}));
        }
        let minimum = load_command_min_size(cmd);
        let is_dylib = matches!(cmd, 0xc | 0x20 | 0x8000_0018 | 0x8000_001f | 0x8000_0023);
        if size < minimum {
            if is_dylib {
                dylib_ordinals.push(None);
            }
            b.warnings.push(format!("{} needs at least {minimum} bytes; skipped its fields", load_command_name(cmd)));
            pos = end;
            continue;
        }
        let r = Reader::new(data.get(..end).unwrap_or(&[]));
        let str_at = |field: usize| -> Option<String> {
            let o = r.u32_le(pos + field)? as usize;
            if o < minimum || o >= size {
                return None;
            }
            Some(std::ffi::CStr::from_bytes_until_nul(r.slice(pos + o, size - o)?).ok()?.to_string_lossy().into_owned())
        };
        match cmd {
            0x19 | 0x1 => {
                let (name_o, vm, fo, ns, sh, shsize) = if cmd == 0x19 { (8, 24, 40, 64, 72, 80) } else { (8, 24, 32, 48, 56, 68) };
                let name = r.cstr(pos + name_o, 16).unwrap_or_default();
                let vmaddr = if cmd == 0x19 { r.u64_le(pos + vm) } else { r.u32_le(pos + vm).map(u64::from) }.unwrap_or(0);
                let fileoff = if cmd == 0x19 { r.u64_le(pos + fo) } else { r.u32_le(pos + fo).map(u64::from) }.unwrap_or(0);
                let filesize = if cmd == 0x19 { r.u64_le(pos + fo + 8) } else { r.u32_le(pos + fo + 4).map(u64::from) }.unwrap_or(0);
                segments.push(MSegment { name, vmaddr, fileoff, filesize });
                let nsects = r.u32_le(pos + ns).unwrap_or(0) as usize;
                let complete = (size - sh) / shsize;
                if nsects > complete {
                    b.warnings.push("segment section headers run past the load command; reading complete headers".into());
                }
                for i in 0..nsects.min(256).min(complete) {
                    let o = pos + sh + i * shsize;
                    let (addr, sz, flags_o) = if cmd == 0x19 {
                        (r.u64_le(o + 32), r.u64_le(o + 40), 64)
                    } else {
                        (r.u32_le(o + 32).map(u64::from), r.u32_le(o + 36).map(u64::from), 56)
                    };
                    sections.push(MSection {
                        sectname: r.cstr(o, 16).unwrap_or_default(),
                        addr: addr.unwrap_or(0),
                        size: sz.unwrap_or(0),
                        flags: r.u32_le(o + flags_o).unwrap_or(0),
                        reserved1: r.u32_le(o + flags_o + 4).unwrap_or(0),
                        reserved2: r.u32_le(o + flags_o + 8).unwrap_or(0),
                    });
                }
            }
            0x2 => {
                symtab =
                    Some((r.u32_le(pos + 8).unwrap_or(0), r.u32_le(pos + 12).unwrap_or(0), r.u32_le(pos + 16).unwrap_or(0), r.u32_le(pos + 20).unwrap_or(0)))
            }
            0xb => indirect = Some((r.u32_le(pos + 56).unwrap_or(0), r.u32_le(pos + 60).unwrap_or(0))),
            _ if is_dylib => dylib_ordinals.push(str_at(8)),
            0xd => {
                if let Some(s) = str_at(8) {
                    b.info.insert("install_name".into(), s);
                }
            }
            0xe => {
                if let Some(s) = str_at(8) {
                    b.info.insert("interpreter".into(), s);
                }
            }
            0x8000_001c => {
                if let Some(s) = str_at(8) {
                    let prev = b.info.remove("rpath");
                    b.info.insert(
                        "rpath".into(),
                        match prev {
                            Some(p) => format!("{p}:{s}"),
                            None => s,
                        },
                    );
                }
            }
            0x8000_0028 => entryoff = r.u64_le(pos + 8),
            0x5 => {
                thread_pc = thread_pc.or_else(|| unixthread_pc(data.get(pos + 8..end)?, r.u32_le(4)?, is64));
            }
            0x26 => fstarts = Some((r.u32_le(pos + 8).unwrap_or(0), r.u32_le(pos + 12).unwrap_or(0))),
            0x1d => b.mitigations.code_signature = Some(true),
            0x21 | 0x2c => b.mitigations.encrypted = Some(r.u32_le(pos + 16).unwrap_or(0) != 0),
            0x32 => {
                // One platform table for the binary and the dependency views.
                b.info.insert("platform".into(), crate::macho_deps::platform_name(r.u32_le(pos + 8).unwrap_or(0)));
                b.info.insert("min_os".into(), nibble_version(r.u32_le(pos + 12).unwrap_or(0)));
                b.info.insert("sdk".into(), nibble_version(r.u32_le(pos + 16).unwrap_or(0)));
            }
            0x24 | 0x25 => {
                b.info.insert("platform".into(), if cmd == 0x24 { "macos" } else { "ios" }.into());
                b.info.insert("min_os".into(), nibble_version(r.u32_le(pos + 8).unwrap_or(0)));
            }
            0x8000_0034 => {
                b.info.insert("fixups".into(), "chained fixups".into());
            }
            0x22 | 0x8000_0022 => {
                b.info.insert("fixups".into(), "dyld info (rebase/bind opcodes)".into());
            }
            _ => {}
        }
        pos = end;
    }
    if b.mitigations.code_signature.is_none() {
        b.mitigations.code_signature = Some(false);
    }
    b.mitigations.pie = Some(header_flags & 0x0020_0000 != 0);
    b.mitigations.nx = Some(header_flags & 0x0002_0000 == 0);
    let text = segments.iter().find(|s| s.name == "__TEXT").cloned();
    // Entry point. LC_MAIN gives a file offset: map it through the segment whose file bytes hold
    // it. Segments with no file bytes (__PAGEZERO, a __DATA of only zero-fill sections) usually
    // record file offset 0, like __TEXT, and must not claim it.
    if let Some(off) = entryoff {
        b.entry = segments.iter().find_map(|s| off.checked_sub(s.fileoff).filter(|&d| d < s.filesize).map(|d| s.vmaddr.wrapping_add(d)));
    } else if let Some(pc) = thread_pc {
        b.entry = Some(pc);
    } else {
        b.entry = None;
    }
    if let Some(t) = &text {
        b.image_base = t.vmaddr;
    }
    // Function starts: ULEB deltas from the start of __TEXT.
    if let (Some((off, size)), Some(t)) = (fstarts, &text) {
        let start = off as usize;
        let end = (off as usize).saturating_add(size as usize).min(data.len());
        let starts = Reader::new(data.get(start..end).unwrap_or(&[]));
        let mut o = 0;
        let mut addr = t.vmaddr;
        while o < starts.len() && b.function_starts.len() < 2_000_000 {
            let Some((delta, n)) = starts.uleb128(o) else {
                b.warnings.push("function-start delta is cut short or invalid".into());
                break;
            };
            if delta == 0 {
                break;
            }
            let Some(next) = addr.checked_add(delta) else {
                b.warnings.push("function-start address overflows".into());
                break;
            };
            addr = next;
            b.function_starts.push(addr);
            o += n;
        }
    }
    // Stubs and pointer slots from the indirect symbol table.
    if let (Some((symoff, nsyms, stroff, strsize)), Some((indoff, nind))) = (symtab, indirect) {
        let nlist_size = if is64 { 16 } else { 12 };
        let sym_name = |idx: u32| -> Option<(String, u16)> {
            if idx >= nsyms {
                return None;
            }
            let e = (symoff as usize).checked_add(idx as usize * nlist_size)?;
            let strx = r.u32_le(e)?;
            if strx >= strsize {
                return None;
            }
            let desc = r.u16_le(e + 6)?;
            Some((r.cstr(stroff as usize + strx as usize, (strsize - strx) as usize)?, desc))
        };
        let mut remaining = MAX_INDIRECT;
        'sections: for s in &sections {
            let ty = s.flags & 0xff;
            let (entsize, kind) = match ty {
                6 | 7 | 0x10 => (if is64 { 8 } else { 4 }, "slot"),
                8 => (u64::from(s.reserved2), "stub"),
                _ => continue,
            };
            if entsize == 0 {
                continue;
            }
            let count = usize::try_from(s.size / entsize).unwrap_or(usize::MAX);
            for i in 0..count {
                let Some(ii) = (s.reserved1 as usize).checked_add(i) else { break };
                if ii >= nind as usize {
                    break;
                }
                if remaining == 0 {
                    b.warnings.push("indirect symbol limit reached across sections; remaining slots omitted".into());
                    break 'sections;
                }
                remaining -= 1;
                let Some(sym) = r.u32_le(indoff as usize + ii * 4) else { break };
                if sym & 0xc000_0000 != 0 {
                    continue;
                }
                let Some((name, desc)) = sym_name(sym) else { continue };
                let ord = usize::from(desc >> 8);
                let library = if ord > 0 && ord < 0xfe { dylib_ordinals.get(ord - 1).cloned().flatten() } else { None };
                b.import_slots.push(ImportSlot { addr: s.addr.wrapping_add(i as u64 * entsize), name, library, kind });
            }
        }
    }
    b.libraries = dylib_ordinals.into_iter().flatten().collect();
    b.details = json!({
        "file_type": match r.u32_le(12) { Some(1) => "object", Some(2) => "execute", Some(6) => "dylib", Some(8) => "bundle", Some(4) => "core", _ => "other" },
        "flags": format!("{header_flags:#x}"),
        "load_commands": cmds_json,
        "segments": segments.iter().map(|s| json!({"name": s.name, "vmaddr": format!("{:#x}", s.vmaddr), "fileoff": s.fileoff})).collect::<Vec<_>>(),
        "sections": sections.iter().map(|s| json!({"name": s.sectname, "type": s.flags & 0xff, "flags": format!("{:#x}", s.flags)})).collect::<Vec<_>>(),
    });
}

// ---------------------------------------------------------------------------------------------
// Objective-C

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObjcMethod {
    pub selector: String,
    /// `-` instance method, `+` class method.
    pub kind: &'static str,
    #[serde(with = "hex_addr::opt")]
    pub imp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub types: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObjcClass {
    pub name: String,
    #[serde(with = "hex_addr")]
    pub addr: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub superclass: Option<String>,
    /// Set for categories: the category's own name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub methods: Vec<ObjcMethod>,
    pub swift: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ObjcProtocol {
    pub name: String,
    #[serde(with = "hex_addr")]
    pub addr: u64,
    pub methods: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ObjcMetadata {
    pub classes: Vec<ObjcClass>,
    pub protocols: Vec<ObjcProtocol>,
    pub selectors: Vec<String>,
    pub warnings: Vec<String>,
}

/// Turn a stored pointer into an address: plain pointers pass through; chained-fixup rebases
/// (36-bit targets, or offsets from the image base) are decoded best effort.
fn decode_ptr(b: &Binary, raw: u64) -> Option<u64> {
    if raw == 0 {
        return None;
    }
    // __PAGEZERO is mapped but has no file bytes. A small chained target
    // offset inside it must still reach the image-base fallback below.
    if b.va_to_offset(raw).is_some() {
        return Some(raw);
    }
    if raw >> 63 != 0 {
        // authenticated rebase: low 32 bits are an offset from the image base
        let t = b.image_base.wrapping_add(raw & 0xffff_ffff);
        return b.va_to_offset(t).is_some().then_some(t);
    }
    let t36 = raw & 0xf_ffff_ffff;
    if b.va_to_offset(t36).is_some() {
        return Some(t36);
    }
    let t = b.image_base.wrapping_add(t36);
    b.va_to_offset(t).is_some().then_some(t)
}

fn section_words(data: &[u8], b: &Binary, names: &[&str]) -> Vec<u64> {
    let mut out = Vec::new();
    for s in b.sections.iter().filter(|s| names.contains(&s.name.as_str())) {
        let n = (s.size / 8).min(MAX_OBJC as u64);
        for i in 0..n {
            if let Some(p) = b.read_ptr(data, s.addr + i * 8).and_then(|raw| decode_ptr(b, raw)) {
                out.push(p);
            }
        }
    }
    out
}

fn cstr_at(data: &[u8], b: &Binary, va: u64) -> Option<String> {
    let bytes = b.read(data, va, 1024)?;
    let end = bytes.iter().position(|&c| c == 0).unwrap_or(bytes.len());
    Some(String::from_utf8_lossy(bytes.get(..end)?).into_owned())
}

fn ptr_at(data: &[u8], b: &Binary, va: u64) -> Option<u64> {
    b.read_ptr(data, va).and_then(|raw| decode_ptr(b, raw))
}

fn method_list(data: &[u8], b: &Binary, list: u64, kind: &'static str, out: &mut Vec<ObjcMethod>) {
    let Some(head) = b.read(data, list, 8) else { return };
    let r = Reader::new(head);
    let (Some(ent_flags), Some(count)) = (r.u32_le(0), r.u32_le(4)) else { return };
    let relative = ent_flags & 0x8000_0000 != 0;
    let entsize = u64::from(ent_flags & 0xfffc);
    if entsize < if relative { 12 } else { 24 } {
        return;
    }
    for i in 0..u64::from(count).min(MAX_METHODS as u64) {
        let Some(e) = i.checked_mul(entsize).and_then(|offset| list.checked_add(8)?.checked_add(offset)) else { break };
        let Some(raw) = b.read(data, e, entsize as usize).filter(|bytes| bytes.len() as u64 == entsize) else { break };
        if relative {
            let rr = Reader::new(raw);
            let rel = |o: usize| rr.u32_le(o).and_then(|v| e.checked_add(o as u64).map(|a| a.wrapping_add(i64::from(v as i32) as u64)));
            let name_field = rel(0);
            // The name offset points at a selector reference unless selectors are direct.
            let selector = if ent_flags & 0x4000_0000 != 0 {
                name_field.and_then(|a| cstr_at(data, b, a))
            } else {
                name_field.and_then(|a| ptr_at(data, b, a)).and_then(|a| cstr_at(data, b, a))
            };
            let Some(selector) = selector else { continue };
            out.push(ObjcMethod { selector, kind, imp: rel(8).filter(|a| b.is_mapped(*a)), types: rel(4).and_then(|a| cstr_at(data, b, a)) });
        } else {
            let Some(selector) = ptr_at(data, b, e).and_then(|a| cstr_at(data, b, a)) else { continue };
            let (Some(types_field), Some(imp_field)) = (e.checked_add(8), e.checked_add(16)) else { continue };
            let types = ptr_at(data, b, types_field).and_then(|a| cstr_at(data, b, a));
            out.push(ObjcMethod { selector, kind, imp: ptr_at(data, b, imp_field), types });
        }
    }
}

fn class_ro(data: &[u8], b: &Binary, class: u64) -> Option<u64> {
    let raw = b.read_ptr(data, class + 32)?;
    decode_ptr(b, raw & !7).or_else(|| decode_ptr(b, raw & 0x0000_7fff_ffff_fff8))
}

fn class_name(data: &[u8], b: &Binary, class: u64) -> Option<String> {
    let ro = class_ro(data, b, class)?;
    ptr_at(data, b, ro + 0x18).and_then(|a| cstr_at(data, b, a))
}

/// Objective-C classes, categories, protocols and selectors of a 64-bit Mach-O. `data` is the
/// whole file (the binary's file offsets are absolute).
pub fn objc_metadata(data: &[u8], b: &Binary) -> ObjcMetadata {
    let mut m = ObjcMetadata::default();
    if b.bits != 64 {
        m.warnings.push("Objective-C metadata is decoded for 64-bit images only".into());
        return m;
    }
    let mut seen_classes = BTreeSet::new();
    for class in section_words(data, b, &["__objc_classlist", "__objc_nlclslist"]) {
        if seen_classes.contains(&class) {
            continue;
        }
        if m.classes.len() >= MAX_OBJC {
            break;
        }
        let Some(name) = class_name(data, b, class) else {
            m.warnings.push(format!("class at {class:#x}: unreadable name"));
            continue;
        };
        seen_classes.insert(class);
        let mut methods = Vec::new();
        if let Some(ro) = class_ro(data, b, class)
            && let Some(list) = ptr_at(data, b, ro + 0x20)
        {
            method_list(data, b, list, "-", &mut methods);
        }
        if let Some(meta) = ptr_at(data, b, class)
            && let Some(ro) = class_ro(data, b, meta)
            && let Some(list) = ptr_at(data, b, ro + 0x20)
        {
            method_list(data, b, list, "+", &mut methods);
        }
        let superclass = ptr_at(data, b, class + 8).and_then(|s| class_name(data, b, s));
        let swift = b.read_ptr(data, class + 32).is_some_and(|raw| raw & 3 != 0) || name.starts_with("_Tt");
        m.classes.push(ObjcClass { name, addr: class, superclass, category: None, methods, swift });
    }
    let mut seen_categories = BTreeSet::new();
    for cat in section_words(data, b, &["__objc_catlist", "__objc_nlcatlist"]) {
        if seen_categories.contains(&cat) {
            continue;
        }
        if m.classes.len() >= MAX_OBJC {
            break;
        }
        let Some(cat_name) = ptr_at(data, b, cat).and_then(|a| cstr_at(data, b, a)) else { continue };
        seen_categories.insert(cat);
        let cls = ptr_at(data, b, cat + 8).and_then(|c| class_name(data, b, c)).unwrap_or_else(|| "(external class)".into());
        let mut methods = Vec::new();
        if let Some(list) = ptr_at(data, b, cat + 16) {
            method_list(data, b, list, "-", &mut methods);
        }
        if let Some(list) = ptr_at(data, b, cat + 24) {
            method_list(data, b, list, "+", &mut methods);
        }
        m.classes.push(ObjcClass { name: cls, addr: cat, superclass: None, category: Some(cat_name), methods, swift: false });
    }
    for p in section_words(data, b, &["__objc_protolist"]) {
        if m.protocols.len() >= MAX_OBJC {
            break;
        }
        let Some(name) = ptr_at(data, b, p + 8).and_then(|a| cstr_at(data, b, a)) else { continue };
        let mut methods = Vec::new();
        for (field, kind) in [(24u64, "-"), (32, "+"), (40, "-"), (48, "+")] {
            if let Some(list) = ptr_at(data, b, p + field) {
                method_list(data, b, list, kind, &mut methods);
            }
        }
        m.protocols.push(ObjcProtocol { name, addr: p, methods: methods.into_iter().map(|x| x.selector).collect() });
    }
    for sel in section_words(data, b, &["__objc_selrefs"]) {
        if m.selectors.len() >= MAX_OBJC {
            break;
        }
        if let Some(s) = cstr_at(data, b, sel) {
            m.selectors.push(s);
        }
    }
    m.protocols.sort_by(|a, b| a.name.cmp(&b.name));
    m.protocols.dedup_by(|a, b| a.name == b.name);
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_testkit as tk;

    fn executable_with_header_symbol(helper: Option<u8>) -> Vec<u8> {
        let mut symtab = vec![0u8; 24];
        symtab[0..4].copy_from_slice(&2u32.to_le_bytes());
        symtab[4..8].copy_from_slice(&24u32.to_le_bytes());
        let mut m = tk::apple::macho_min(tk::apple::CPU_ARM64, 2, &[symtab], &[("__text", &[0; 4])]);
        let symoff = m.len() as u32;
        let names = b"\0__mh_execute_header\0_internal_helper\0";
        let count = 1 + u32::from(helper.is_some());
        for (name, kind, desc) in [(1u32, 0x0fu8, 0x10u16), (21, helper.unwrap_or(0x0e), 0)].into_iter().take(count as usize) {
            m.extend_from_slice(&name.to_le_bytes());
            m.extend_from_slice(&[kind, 1]);
            m.extend_from_slice(&desc.to_le_bytes());
            m.extend_from_slice(&0x1_0000_0000u64.to_le_bytes());
        }
        let stroff = m.len() as u32;
        m.extend_from_slice(names);
        let cmd = 32 + 72 + 80;
        for (off, value) in [(8, symoff), (12, count), (16, stroff), (20, names.len() as u32)] {
            m[cmd + off..cmd + off + 4].copy_from_slice(&value.to_le_bytes());
        }
        m.resize(0x4000, 0);
        m
    }

    #[test]
    fn an_executable_with_only_its_linker_header_is_stripped() {
        let control = crate::parse_binary(&executable_with_header_symbol(Some(0x0e))).unwrap();
        assert!(control.symbols.iter().any(|s| s.name == "_internal_helper" && !s.global && s.defined));
        assert!(!control.mitigations.stripped);
        // An unstripped executable whose own functions are all external keeps their names.
        let global = crate::parse_binary(&executable_with_header_symbol(Some(0x0f))).unwrap();
        assert!(global.symbols.iter().any(|s| s.name == "_internal_helper" && s.global && s.defined));
        assert!(!global.mitigations.stripped);
        let b = crate::parse_binary(&executable_with_header_symbol(None)).unwrap();
        assert_eq!(b.symbols.len(), 1);
        let s = &b.symbols[0];
        assert_eq!(s.name, "__mh_execute_header");
        assert!(s.global && s.defined && !s.dynamic);
        assert!(b.mitigations.stripped);
    }

    #[test]
    fn a_haswell_request_selects_the_haswell_slice() {
        use crate::macho_deps::{pick_slice, slice_bytes, slices};
        let mut generic = tk::apple::macho_min(tk::apple::CPU_X86_64, 1, &[], &[("__text", &[0xc3])]);
        generic[8..12].copy_from_slice(&3u32.to_le_bytes());
        let mut haswell = generic.clone();
        haswell[8..12].copy_from_slice(&8u32.to_le_bytes());
        for order in [vec![generic.clone(), haswell.clone()], vec![haswell.clone(), generic.clone()]] {
            let fat = tk::macho_fat(&order);
            let all = slices(&fat).unwrap();
            let selected = pick_slice(&all, Some("x86_64h")).unwrap();
            assert_eq!(selected.cpusubtype, 8);
            assert_eq!(slice_bytes(&fat, &selected).unwrap(), haswell);
            assert_eq!(pick_slice(&all, Some("x86_64")).unwrap().cpusubtype, 3);
            assert!(pick_slice(&all, Some("unknown-architecture")).is_none());
        }
        assert_eq!(slices(&haswell).unwrap()[0].arch, "x86_64h");
        assert_eq!(pick_slice(&slices(&generic).unwrap(), Some("x86_64h")).unwrap().cpusubtype, 3);
        assert_eq!(cpu_name(tk::apple::CPU_X86_64, 0x8000_0008), "x86_64h");
        assert_eq!(cpu_name(tk::apple::CPU_X86_64, 0x108), "x86_64");
    }

    #[test]
    fn thin_macho_facts() {
        let m = tk::macho64();
        let b = crate::parse_binary(&m).unwrap();
        assert_eq!(b.entry, Some(tk::MACHO_BASE + tk::MACHO_TEXT_OFF));
        assert_eq!(b.function_starts.len(), 4);
        assert!(b.function_starts.contains(&(tk::MACHO_BASE + tk::MACHO_TEXT_OFF + tk::x64::HIDDEN)));
        assert_eq!(b.libraries, vec!["/usr/lib/libSystem.B.dylib".to_string()]);
        let slot = b.import_slots.iter().find(|s| s.name == "_puts").unwrap();
        assert_eq!(slot.addr, tk::MACHO_BASE + 0x1000);
        assert_eq!(slot.library.as_deref(), Some("/usr/lib/libSystem.B.dylib"));
        assert_eq!(b.mitigations.pie, Some(true));
    }

    /// An executable laid out like `/bin/cat` on arm64e: `__TEXT` maps the first 16 KiB of the
    /// file, and `__DATA` holds only zero-fill sections, so it has no file bytes and its file
    /// offset is 0, the same as `__TEXT`'s.
    fn executable_with_zero_fill_data(entryoff: u64) -> Vec<u8> {
        let mut data = vec![0u8; 72];
        data[0..4].copy_from_slice(&0x19u32.to_le_bytes());
        data[4..8].copy_from_slice(&72u32.to_le_bytes());
        data[8..14].copy_from_slice(b"__DATA");
        data[24..32].copy_from_slice(&(tk::MACHO_BASE + 0x4000).to_le_bytes());
        data[32..40].copy_from_slice(&0x4000u64.to_le_bytes());
        data[56..60].copy_from_slice(&3u32.to_le_bytes());
        data[60..64].copy_from_slice(&3u32.to_le_bytes());
        let mut main = vec![0u8; 24];
        main[0..4].copy_from_slice(&0x8000_0028u32.to_le_bytes());
        main[4..8].copy_from_slice(&24u32.to_le_bytes());
        main[8..16].copy_from_slice(&entryoff.to_le_bytes());
        let mut m = tk::apple::macho_min(tk::apple::CPU_ARM64, 2, &[data, main], &[("__text", &[0; 4])]);
        m.resize(0x4000, 0);
        m
    }

    #[test]
    fn the_entry_point_is_in_the_segment_that_holds_its_file_bytes() {
        let b = crate::parse_binary(&executable_with_zero_fill_data(0x588)).unwrap();
        let layout: Vec<_> = b.segments.iter().map(|s| (s.name.as_str(), s.file_offset, s.file_size)).collect();
        assert_eq!(layout, [("__TEXT", 0, 0x4000), ("__DATA", 0, 0)]);
        assert_eq!(b.entry, Some(tk::MACHO_BASE + 0x588));
        // An offset past every segment's file bytes names no code.
        assert_eq!(crate::parse_binary(&executable_with_zero_fill_data(0x5000)).unwrap().entry, None);
    }

    #[test]
    fn objc_classes_and_protocols() {
        let m = tk::macho64();
        let b = crate::parse_binary(&m).unwrap();
        let meta = objc_metadata(&m, &b);
        assert_eq!(meta.classes.len(), 1, "{meta:?}");
        let c = &meta.classes[0];
        assert_eq!(c.name, tk::OBJC_CLASS);
        assert_eq!(c.methods[0].selector, tk::OBJC_SELECTOR);
        assert_eq!(c.methods[0].imp, Some(tk::MACHO_BASE + tk::MACHO_TEXT_OFF + tk::x64::HIDDEN));
        assert_eq!(meta.protocols[0].name, tk::OBJC_PROTOCOL);
    }

    #[test]
    fn objc_method_list_address_overflow_never_panics() {
        let mut m = tk::macho64();
        let list = u64::MAX - 8;
        let fileoff = m.len() as u64;
        // Reuse the sectionless __PAGEZERO command for an eight-byte, file-backed segment.
        m[32 + 24..32 + 32].copy_from_slice(&list.to_le_bytes());
        m[32 + 32..32 + 40].copy_from_slice(&8u64.to_le_bytes());
        m[32 + 40..32 + 48].copy_from_slice(&fileoff.to_le_bytes());
        m[32 + 48..32 + 56].copy_from_slice(&8u64.to_le_bytes());
        m.extend_from_slice(&24u32.to_le_bytes());
        m.extend_from_slice(&2u32.to_le_bytes());
        m[0x1060..0x1068].copy_from_slice(&list.to_le_bytes()); // class_ro.baseMethods

        let b = crate::parse_binary(&m).unwrap();
        assert!(b.is_mapped(list));
        assert_eq!(ptr_at(&m, &b, tk::MACHO_BASE + 0x1060), Some(list));
        assert_eq!(b.read(&m, list, 8).unwrap().len(), 8);
        let meta = objc_metadata(&m, &b);
        assert_eq!(meta.classes.len(), 1);
        assert!(meta.classes[0].methods.is_empty());
    }

    #[test]
    fn objc_chained_offsets_do_not_resolve_into_pagezero() {
        let mut data = tk::macho64();
        let b = crate::parse_binary(&data).unwrap();
        let list = b.sections.iter().find(|s| s.name == "__objc_classlist").unwrap().addr;
        let class = b.read_ptr(&data, list).unwrap();
        let raw_ro = b.read_ptr(&data, class + 32).unwrap();
        for (slot, target) in [(list, class), (class + 32, raw_ro)] {
            let off = b.va_to_offset(slot).unwrap() as usize;
            let encoded = (1u64 << 52) | (target - b.image_base);
            data[off..off + 8].copy_from_slice(&encoded.to_le_bytes());
        }
        assert!(b.is_mapped(class - b.image_base));
        assert!(b.va_to_offset(class - b.image_base).is_none());
        let meta = objc_metadata(&data, &b);
        assert_eq!(meta.classes.len(), 1, "{meta:?}");
        assert_eq!(meta.classes[0].name, tk::OBJC_CLASS);
        assert_eq!(meta.classes[0].methods[0].selector, tk::OBJC_SELECTOR);
        assert_eq!(meta.classes[0].methods[0].imp, Some(tk::MACHO_BASE + tk::MACHO_TEXT_OFF + tk::x64::HIDDEN));
        let fat = tk::macho_fat(&[data]);
        let fat_binary = crate::parse_binary(&fat).unwrap();
        let fat_meta = objc_metadata(&fat, &fat_binary);
        assert_eq!(fat_meta.classes.len(), 1, "{fat_meta:?}");
        assert_eq!(fat_meta.classes[0].methods[0].selector, tk::OBJC_SELECTOR);
        assert_eq!(decode_ptr(&b, class), Some(class));
        let offset = class - b.image_base;
        assert_eq!(decode_ptr(&b, offset), Some(class));
        assert_eq!(decode_ptr(&b, (1u64 << 63) | offset), Some(class));
        assert_eq!(decode_ptr(&b, u64::MAX), None);
        assert_eq!(decode_ptr(&b, b.image_base + 0x0fff_ffff), None);
        assert_eq!(decode_ptr(&b, 0), None);
    }

    #[test]
    fn fat_slices_and_parse() {
        let thin = tk::macho64();
        let fat = tk::macho_fat(std::slice::from_ref(&thin));
        let s = fat_slices(&fat).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].arch, "x86_64");
        let b = crate::parse_binary(&fat).unwrap();
        assert_eq!(b.slice_offset, 0x1000);
        let text = b.sections.iter().find(|s| s.name == "__text").unwrap();
        assert_eq!(text.file_offset, Some(0x1000 + tk::MACHO_TEXT_OFF));
        // Objective-C reads go through absolute offsets.
        assert_eq!(objc_metadata(&fat, &b).classes.len(), 1);
    }

    #[test]
    fn damaged_machos_never_panic() {
        let m = tk::macho64();
        for d in tk::mutations(&m) {
            if let Ok(b) = crate::parse_binary(&d) {
                let _ = objc_metadata(&d, &b);
            }
        }
        for d in tk::mutations(&tk::macho_fat(&[m])) {
            let _ = fat_slices(&d);
            let _ = crate::parse_binary(&d);
        }
    }

    #[test]
    fn read_only_segments_keep_stub_and_unwind_sections_read_only() {
        let mut m = tk::apple::macho_min(tk::apple::CPU_ARM64, 2, &[], &[("__text", &[0; 4]), ("__stubs", &[0; 12]), ("__unwind_info", &[0; 8])]);
        m[32 + 72 + 64..32 + 72 + 68].copy_from_slice(&0x8000_0400u32.to_le_bytes());
        m[32 + 72 + 80 + 64..32 + 72 + 80 + 68].copy_from_slice(&0x8000_0408u32.to_le_bytes());
        m.resize(0x4000, 0);
        let b = crate::parse_binary(&m).unwrap();
        assert_eq!(b.sections.len(), 3);
        assert!(!b.segments[0].write);
        for s in &b.sections {
            assert!(s.read);
            assert!(!s.write, "{} must inherit its read-only segment", s.name);
        }
        assert!(b.sections[0].exec);
        assert!(b.sections[1].exec);
        assert!(!b.sections[2].exec);
        m[32 + 56..32 + 60].copy_from_slice(&7u32.to_le_bytes());
        m[32 + 60..32 + 64].copy_from_slice(&3u32.to_le_bytes());
        let writable = crate::parse_binary(&m).unwrap();
        assert!(writable.sections.iter().all(|s| s.read && s.write));
        assert!(writable.sections[1].exec);
        assert!(!writable.sections[2].exec);
    }
}

#[cfg(test)]
mod shared_parity {
    use super::*;
    fn compare(data: &[u8]) {
        match (legacy::fat_slices(data), fat_slices(data)) {
            (Ok(a), Ok(b)) => assert_eq!(serde_json::to_value(a).unwrap(), serde_json::to_value(b).unwrap()),
            (Err(a), Err(b)) => assert_eq!(a, b),
            other => panic!("fat parity mismatch: {other:?}"),
        }
    }
    #[test]
    fn shared_fat_errors_bounds_and_serialization_match() {
        for wide in [false, true] {
            for count in [0u32, 1, 64, 65, u32::MAX] {
                let mut data = vec![0; 4096];
                data[..4].copy_from_slice(&if wide { 0xcafe_babfu32 } else { 0xcafe_babeu32 }.to_be_bytes());
                data[4..8].copy_from_slice(&count.to_be_bytes());
                compare(&data);
                for len in 0..150 {
                    compare(&data[..len]);
                }
                for pos in 0..100 {
                    let old = data[pos];
                    data[pos] = 255;
                    compare(&data);
                    data[pos] = old;
                }
            }
        }
    }
    #[test]
    fn shared_names_and_thread_states_match() {
        for n in 0..512u32 {
            for v in [n, n | 0x8000_0000, u32::MAX] {
                assert_eq!(legacy::load_command_name(v), load_command_name(v));
                assert_eq!(legacy::load_command_min_size(v), load_command_min_size(v));
                assert_eq!(legacy::nibble_version(v), nibble_version(v));
                for cpu in [7, 12, 0x0100_0007, 0x0100_000c, 0x0200_000c, 18, 0x0100_0012, v] {
                    assert_eq!(legacy::cpu_name(cpu, v), cpu_name(cpu, v));
                }
            }
        }
        for (cpu, wide, flavor, count) in [(7, false, 1u32, 16u32), (0x0100_0007, true, 4, 42), (0x0100_000c, true, 6, 68)] {
            let mut data = vec![0; 8 + count as usize * 4];
            data[..4].copy_from_slice(&flavor.to_le_bytes());
            data[4..8].copy_from_slice(&count.to_le_bytes());
            data[8..].fill(31);
            for len in 0..=data.len() {
                assert_eq!(legacy::unixthread_pc(&data[..len], cpu, wide), unixthread_pc(&data[..len], cpu, wide));
            }
            for records in [4095, 4096] {
                let mut prefixed = vec![0; records * 8];
                prefixed.extend_from_slice(&data);
                assert_eq!(legacy::unixthread_pc(&prefixed, cpu, wide), unixthread_pc(&prefixed, cpu, wide));
                assert_eq!(unixthread_pc(&prefixed, cpu, wide).is_some(), records == 4095);
            }
            data[4..8].fill(255);
            assert_eq!(legacy::unixthread_pc(&data, cpu, wide), unixthread_pc(&data, cpu, wide));
        }
    }
}
