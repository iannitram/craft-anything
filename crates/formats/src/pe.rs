//! PE details: headers, imports with their IAT slots, unwind-table function starts, TLS
//! callbacks, the Rich header, mitigations, and the resource tree (icons, version info,
//! manifest).

use crate::binary::{Binary, Import, ImportSlot};
use crate::bytes::Reader;
use crate::{FormatError, hex_addr};
use serde::Serialize;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

const MAX_IMPORT_DESCRIPTORS: usize = 4096;
const MAX_THUNKS: usize = 65_536;
const MAX_UNWIND: usize = 500_000;
/// Most imported functions kept from one file, across every library.
const MAX_IMPORT_ROWS: usize = 200_000;
/// Most exported entries kept from one file.
const MAX_EXPORTS: usize = 200_000;
/// Most decoded name bytes retained by an import or export table by default.
pub const DEFAULT_MAX_TABLE_TEXT_BYTES: usize = 64 << 20;

struct TableTextBudget {
    remaining: usize,
    exhausted: bool,
}

impl TableTextBudget {
    fn new(bytes: usize) -> Self {
        Self { remaining: bytes, exhausted: false }
    }

    fn take(&mut self, bytes: usize) -> bool {
        if self.exhausted || bytes > self.remaining {
            self.exhausted = true;
            return false;
        }
        self.remaining -= bytes;
        true
    }

    fn finish(&self, truncated: &mut bool, warnings: &mut Vec<String>) {
        if self.exhausted {
            *truncated = true;
            warnings.push("the table's retained name-byte budget was reached; later names were omitted".into());
        }
    }
}

pub use craft_pe::{PeHeaders, PeSection, has_pe_header, rich_header, subsystem_name};

/// Read PE headers while preserving the app error contract.
pub fn headers(data: &[u8]) -> Result<PeHeaders, FormatError> {
    craft_pe::headers(data).map_err(|error| match error {
        craft_pe::PeError::Truncated(context) => FormatError::Truncated(context.into()),
        craft_pe::PeError::Malformed(context) => FormatError::Malformed(context),
    })
}

#[cfg(test)]
#[path = "pe_legacy.rs"]
mod legacy;

const DIR_NAMES: [&str; 16] = [
    "export",
    "import",
    "resource",
    "exception",
    "security",
    "basereloc",
    "debug",
    "architecture",
    "globalptr",
    "tls",
    "load_config",
    "bound_import",
    "iat",
    "delay_import",
    "clr_runtime",
    "reserved",
];

/// One import with its IAT slot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeImport {
    pub library: String,
    pub name: Option<String>,
    pub ordinal: Option<u16>,
    pub slot_va: u64,
}

/// Walk the import directory, retaining functions with representable IAT slot addresses.
pub fn imports(data: &[u8], h: &PeHeaders) -> Vec<PeImport> {
    let mut table = ImportTable::default();
    normal_imports(data, h, &mut table, &mut TableTextBudget::new(DEFAULT_MAX_TABLE_TEXT_BYTES));
    table.rows.into_iter().filter_map(|row| Some(PeImport { slot_va: row.slot_va?, library: row.library, name: row.name, ordinal: row.ordinal })).collect()
}

/// How a program links to an imported function.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportKind {
    /// Through the import directory: the loader resolves it when the program starts.
    Normal,
    /// Through the delay-load directory: resolved the first time it is called.
    Delay,
}

impl ImportKind {
    pub fn name(self) -> &'static str {
        match self {
            ImportKind::Normal => "normal",
            ImportKind::Delay => "delay",
        }
    }
}

/// One imported function, by name or by ordinal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImportRow {
    pub library: String,
    pub kind: ImportKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ordinal: Option<u16>,
    /// The linker's guess at where the name sits in the library's export table.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<u16>,
    /// The pointer slot the loader (or delay-load helper) fills.
    #[serde(with = "hex_addr::opt", skip_serializing_if = "Option::is_none")]
    pub slot_va: Option<u64>,
    /// The file records addresses already resolved against one version of the library.
    pub bound: bool,
}

/// A library the bound import directory says this file was pre-resolved against.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BoundModule {
    pub name: String,
    pub timestamp: u32,
    pub forwarders: Vec<String>,
}

/// Everything the import, delay-load and bound import directories say.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ImportTable {
    pub rows: Vec<ImportRow>,
    pub bound_modules: Vec<BoundModule>,
    /// A cap or incomplete directory stopped the walk: more rows may exist.
    pub truncated: bool,
    pub warnings: Vec<String>,
}

fn thunk_at(r: &Reader<'_>, h: &PeHeaders, off: usize) -> Option<u64> {
    if h.pe32_plus { r.u64_le(off) } else { r.u32_le(off).map(u64::from) }
}

/// One thunk as a row's name, hint and ordinal. `name_rva` turns the thunk's value into an RVA.
fn thunk_target(r: &Reader<'_>, h: &PeHeaders, v: u64, name_rva: impl Fn(u64) -> Option<u32>) -> (Option<String>, Option<u16>, Option<u16>) {
    let by_ordinal = if h.pe32_plus { v & (1 << 63) != 0 } else { v & (1 << 31) != 0 };
    if by_ordinal {
        return (None, None, Some((v & 0xffff) as u16));
    }
    let Some(off) = name_rva(v).and_then(|rva| h.rva_to_offset(rva)) else { return (None, None, None) };
    let hint = r.u16_le(off);
    let name = off.checked_add(2).and_then(|o| r.cstr(o, 512));
    (name, hint, None)
}

fn mapped_import_directory<'a>(data: &'a [u8], h: &PeHeaders, rva: u32, size: u32, label: &str, out: &mut ImportTable) -> Option<Reader<'a>> {
    let Some((start, mapped)) = h.rva_to_offset(rva).zip(h.rva_file_len(rva)) else {
        out.truncated = true;
        out.warnings.push(format!("the {label} directory at RVA {rva:#x} is outside the file"));
        return None;
    };
    let available = data.get(start..).unwrap_or_default();
    let len = mapped.min(available.len());
    if len < size as usize {
        out.truncated = true;
        out.warnings.push(format!("the {label} directory extends beyond its mapped file bytes"));
    }
    Some(Reader::new(available.get(..len).unwrap_or_default()))
}

fn import_directory<'a>(data: &'a [u8], h: &PeHeaders, index: usize, label: &str, out: &mut ImportTable) -> Option<Reader<'a>> {
    let (rva, size) = h.dir(index)?;
    let mapped = mapped_import_directory(data, h, rva, size, label, out)?;
    Some(Reader::new(mapped.slice(0, mapped.len().min(size as usize))?))
}

fn normal_imports(data: &[u8], h: &PeHeaders, out: &mut ImportTable, budget: &mut TableTextBudget) {
    if budget.exhausted {
        return;
    }
    let r = Reader::new(data);
    let Some((rva, size)) = h.data_dirs.get(1).copied().filter(|(rva, _)| *rva != 0) else { return };
    let Some(directory) = mapped_import_directory(data, h, rva, size, "import", out) else { return };
    let thunk_size = if h.pe32_plus { 8usize } else { 4 };
    let mut warned_size = false;
    for d in 0..MAX_IMPORT_DESCRIPTORS {
        let desc = d * 20;
        if directory.slice(desc, 20).is_none() {
            out.truncated = true;
            out.warnings.push("the import descriptor table ends before a complete terminator".into());
            return;
        }
        // Windows follows descriptors to their terminator even when Size is zero or too short.
        // The selected file mapping remains the hard boundary; Size is diagnostic only.
        if desc + 20 > size as usize && !warned_size {
            out.warnings.push("the import descriptor table extends beyond its declared size".into());
            warned_size = true;
        }
        let (Some(oft), Some(stamp), Some(name_rva), Some(ft)) =
            (directory.u32_le(desc), directory.u32_le(desc + 4), directory.u32_le(desc + 12), directory.u32_le(desc + 16))
        else {
            break;
        };
        if oft == 0 && name_rva == 0 && ft == 0 {
            return;
        }
        let library = h.rva_to_offset(name_rva).and_then(|o| r.cstr(o, 256)).unwrap_or_default();
        let bound = stamp != 0;
        if bound && oft == 0 {
            // A bound table with no lookup table: the slots hold addresses, not names.
            out.warnings.push(format!("{library} is bound and has no lookup table, so its function names can't be read"));
            continue;
        }
        let lookup = if oft != 0 { oft } else { ft };
        let Some(first) = h.rva_to_offset(lookup) else { continue };
        for i in 0..MAX_THUNKS {
            if out.rows.len() >= MAX_IMPORT_ROWS {
                out.truncated = true;
                return;
            }
            let Some(t) = i.checked_mul(thunk_size).and_then(|o| first.checked_add(o)) else { break };
            let Some(v) = thunk_at(&r, h, t) else { break };
            if v == 0 {
                break;
            }
            let (name, hint, ordinal) = thunk_target(&r, h, v, |v| u32::try_from(v & 0x7fff_ffff).ok());
            if !budget.take(library.len().saturating_add(name.as_ref().map_or(0, String::len))) {
                return;
            }
            let slot_va = u64::from(ft).checked_add((i * thunk_size) as u64).and_then(|rva| h.image_va(rva));
            out.rows.push(ImportRow { library: library.clone(), kind: ImportKind::Normal, name, ordinal, hint, slot_va, bound });
        }
    }
    out.truncated = true;
    out.warnings.push(format!("only the first {MAX_IMPORT_DESCRIPTORS} import descriptors were read"));
}

/// An RVA held in a delay-load descriptor: old linkers wrote virtual addresses instead.
fn delay_rva(field: u32, rva_based: bool, image_base: u64) -> Option<u32> {
    if rva_based { Some(field) } else { u64::from(field).checked_sub(image_base).and_then(|v| u32::try_from(v).ok()) }
}

fn delay_imports(data: &[u8], h: &PeHeaders, out: &mut ImportTable, budget: &mut TableTextBudget) {
    if budget.exhausted {
        return;
    }
    let r = Reader::new(data);
    let Some(directory) = import_directory(data, h, 13, "delay-load", out) else { return };
    let thunk_size = if h.pe32_plus { 8usize } else { 4 };
    for d in 0..MAX_IMPORT_DESCRIPTORS {
        let desc = d * 32;
        if directory.slice(desc, 32).is_none() {
            out.truncated = true;
            out.warnings.push("the delay-load descriptor table ends before a complete terminator".into());
            return;
        }
        let (Some(attrs), Some(name_f), Some(iat_f), Some(int_f)) =
            (directory.u32_le(desc), directory.u32_le(desc + 4), directory.u32_le(desc + 12), directory.u32_le(desc + 16))
        else {
            break;
        };
        if name_f == 0 && iat_f == 0 && int_f == 0 {
            return;
        }
        let rva_based = attrs & 1 != 0;
        let conv = |f: u32| delay_rva(f, rva_based, h.image_base);
        let library = conv(name_f).and_then(|n| h.rva_to_offset(n)).and_then(|o| r.cstr(o, 256)).unwrap_or_default();
        let Some(first) = conv(int_f).and_then(|n| h.rva_to_offset(n)) else {
            out.warnings.push(format!("the delay-load name table of {library} is outside the file"));
            continue;
        };
        let iat = conv(iat_f);
        for i in 0..MAX_THUNKS {
            if out.rows.len() >= MAX_IMPORT_ROWS {
                out.truncated = true;
                return;
            }
            let Some(t) = i.checked_mul(thunk_size).and_then(|o| first.checked_add(o)) else { break };
            let Some(v) = thunk_at(&r, h, t) else { break };
            if v == 0 {
                break;
            }
            let (name, hint, ordinal) = thunk_target(&r, h, v, |v| {
                if rva_based { u32::try_from(v & 0x7fff_ffff).ok() } else { v.checked_sub(h.image_base).and_then(|x| u32::try_from(x).ok()) }
            });
            let slot_va = iat.and_then(|a| u64::from(a).checked_add((i * thunk_size) as u64)).and_then(|rva| h.image_va(rva));
            if !budget.take(library.len().saturating_add(name.as_ref().map_or(0, String::len))) {
                return;
            }
            out.rows.push(ImportRow { library: library.clone(), kind: ImportKind::Delay, name, ordinal, hint, slot_va, bound: false });
        }
    }
    out.truncated = true;
    out.warnings.push(format!("only the first {MAX_IMPORT_DESCRIPTORS} delay-load descriptors were read"));
}

fn bound_modules(data: &[u8], h: &PeHeaders, out: &mut ImportTable, budget: &mut TableTextBudget) {
    if budget.exhausted {
        return;
    }
    let Some(directory) = import_directory(data, h, 11, "bound import", out) else { return };
    let mut pos = 0usize;
    while out.bound_modules.len() < MAX_IMPORT_DESCRIPTORS {
        if directory.slice(pos, 8).is_none() {
            out.truncated = true;
            out.warnings.push("the bound import table ends before a complete terminator".into());
            return;
        }
        let (Some(stamp), Some(name_off), Some(refs)) = (directory.u32_le(pos), directory.u16_le(pos + 4), directory.u16_le(pos + 6)) else { return };
        if stamp == 0 && name_off == 0 {
            return;
        }
        let name_of = |offset: u16| {
            let offset = usize::from(offset);
            let bytes = directory.slice(offset, directory.len().checked_sub(offset)?.min(256))?;
            let end = bytes.iter().position(|&byte| byte == 0)?;
            Some(String::from_utf8_lossy(bytes.get(..end)?).into_owned())
        };
        let name = name_of(name_off);
        if name.is_none() {
            out.truncated = true;
            out.warnings.push("a bound import name is outside its directory".into());
        }
        if !budget.take(name.as_ref().map_or(0, String::len)) {
            return;
        }
        let refs_start = pos + 8;
        let complete = directory.len().saturating_sub(refs_start) / 8;
        let count = usize::from(refs).min(complete);
        let mut forwarders = Vec::new();
        let mut bad_forwarder = false;
        for i in 0..count {
            let at = refs_start + i * 8;
            if let Some(name) = directory.u16_le(at + 4).and_then(name_of) {
                if !budget.take(name.len()) {
                    break;
                }
                forwarders.push(name);
            } else {
                bad_forwarder = true;
            }
        }
        if bad_forwarder {
            out.truncated = true;
            out.warnings.push("a bound forwarder name is outside its directory".into());
        }
        out.bound_modules.push(BoundModule { name: name.unwrap_or_default(), timestamp: stamp, forwarders });
        if budget.exhausted {
            return;
        }
        if count < usize::from(refs) {
            out.truncated = true;
            out.warnings.push("the bound import forwarder records run past the directory".into());
            return;
        }
        pos = refs_start + count * 8;
    }
    out.truncated = true;
    out.warnings.push(format!("only the first {MAX_IMPORT_DESCRIPTORS} bound import descriptors were read"));
}

/// Read the import, delay-load and bound import directories.
pub fn import_table(data: &[u8], h: &PeHeaders) -> ImportTable {
    import_table_with_text_limit(data, h, DEFAULT_MAX_TABLE_TEXT_BYTES)
}

/// Read imports with one retained-string byte budget shared by normal, delay and bound imports.
/// The budget counts decoded UTF-8 bytes, including repeated library names and forwarders.
pub fn import_table_with_text_limit(data: &[u8], h: &PeHeaders, max_text_bytes: usize) -> ImportTable {
    let mut out = ImportTable::default();
    let mut budget = TableTextBudget::new(max_text_bytes);
    normal_imports(data, h, &mut out, &mut budget);
    delay_imports(data, h, &mut out, &mut budget);
    bound_modules(data, h, &mut out, &mut budget);
    budget.finish(&mut out.truncated, &mut out.warnings);
    out
}

/// One exported entry: by ordinal, with a name when it has one, and either code or a forwarder.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeExport {
    pub ordinal: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub rva: u32,
    /// `OTHER.Name` or `OTHER.#12`: the entry is another library's, and holds no code here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub forwarder: Option<String>,
}

/// The export directory.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ExportTable {
    pub dll_name: Option<String>,
    pub timestamp: u32,
    pub ordinal_base: u32,
    pub declared_functions: u32,
    pub declared_names: u32,
    pub entries: Vec<PeExport>,
    /// A cap or the end of the file stopped the walk: more entries may exist.
    pub truncated: bool,
    pub warnings: Vec<String>,
}

/// Read the export directory; `None` when the file has none.
pub fn export_table(data: &[u8], h: &PeHeaders) -> Option<ExportTable> {
    export_table_with_text_limit(data, h, DEFAULT_MAX_TABLE_TEXT_BYTES)
}

/// Read exports within a decoded-name byte budget, counting the DLL name and every alias copy.
pub fn export_table_with_text_limit(data: &[u8], h: &PeHeaders, max_text_bytes: usize) -> Option<ExportTable> {
    let (dir_rva, dir_size) = h.dir(0)?;
    let r = Reader::new(data);
    let mut out = ExportTable::default();
    let mut budget = TableTextBudget::new(max_text_bytes);
    let Some(d) = h.rva_to_offset(dir_rva) else {
        out.warnings.push(format!("the export directory at RVA {dir_rva:#x} is outside the file"));
        out.truncated = true;
        return Some(out);
    };
    let field = |o: usize| d.checked_add(o).and_then(|p| r.u32_le(p));
    let (Some(name_rva), Some(base), Some(nfuncs), Some(nnames), Some(funcs), Some(names), Some(ords)) =
        (field(12), field(16), field(20), field(24), field(28), field(32), field(36))
    else {
        out.warnings.push("the export directory runs past the end of the file".into());
        out.truncated = true;
        return Some(out);
    };
    out.timestamp = field(4).unwrap_or(0);
    out.ordinal_base = base;
    out.declared_functions = nfuncs;
    out.declared_names = nnames;
    out.dll_name = h.rva_to_offset(name_rva).and_then(|o| r.cstr(o, 512));
    if !budget.take(out.dll_name.as_ref().map_or(0, String::len)) {
        out.dll_name = None;
        budget.finish(&mut out.truncated, &mut out.warnings);
        return Some(out);
    }

    // Keep offsets, not decoded copies: aliases can all refer to the same long string.
    let mut named: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    if let (Some(nt), Some(ot)) = (h.rva_to_offset(names), h.rva_to_offset(ords)) {
        let mut bad_ordinal = 0u32;
        for i in 0..(nnames as usize).min(MAX_EXPORTS) {
            let (Some(n), Some(o)) = (i.checked_mul(4).and_then(|x| nt.checked_add(x)), i.checked_mul(2).and_then(|x| ot.checked_add(x))) else { break };
            let (Some(n), Some(o)) = (r.u32_le(n), r.u16_le(o)) else {
                out.truncated = true;
                break;
            };
            if u32::from(o) >= nfuncs {
                bad_ordinal = bad_ordinal.saturating_add(1);
                continue;
            }
            let Some(offset) = h.rva_to_offset(n).filter(|&p| p <= data.len()) else { continue };
            named.entry(u32::from(o)).or_default().push(offset);
        }
        if bad_ordinal > 0 {
            out.warnings.push(format!("{bad_ordinal} name(s) point past the function table"));
        }
    } else if nnames != 0 {
        out.warnings.push("the export name tables are outside the file, so only ordinals are listed".into());
    }
    if (nnames as usize) > MAX_EXPORTS {
        out.truncated = true;
    }

    let Some(ft) = h.rva_to_offset(funcs) else {
        if nfuncs != 0 {
            out.warnings.push("the export function table is outside the file".into());
            out.truncated = true;
        }
        return Some(out);
    };
    let dir_end = u64::from(dir_rva).saturating_add(u64::from(dir_size));
    if nfuncs as usize > MAX_EXPORTS {
        out.truncated = true;
        out.warnings.push(format!("only the first {MAX_EXPORTS} export function slots were examined"));
    }
    for i in 0..nfuncs.min(MAX_EXPORTS as u32) {
        if out.entries.len() >= MAX_EXPORTS {
            out.truncated = true;
            break;
        }
        let Some(at) = (i as usize).checked_mul(4).and_then(|x| ft.checked_add(x)) else { break };
        let Some(rva) = r.u32_le(at) else {
            out.warnings.push(format!("the export function table ends after {i} of {nfuncs} entries"));
            out.truncated = true;
            break;
        };
        if rva == 0 {
            continue;
        }
        let ordinal = u64::from(base).saturating_add(u64::from(i));
        let forwarder =
            (u64::from(rva) >= u64::from(dir_rva) && u64::from(rva) < dir_end).then(|| h.rva_to_offset(rva).and_then(|o| r.cstr(o, 512)).unwrap_or_default());
        match named.remove(&i) {
            Some(list) => {
                let room = MAX_EXPORTS.saturating_sub(out.entries.len());
                if list.len() > room {
                    out.truncated = true;
                }
                for offset in list.into_iter().take(room) {
                    let Some(name) = r.cstr(offset, 1024) else { continue };
                    if !budget.take(name.len().saturating_add(forwarder.as_ref().map_or(0, String::len))) {
                        budget.finish(&mut out.truncated, &mut out.warnings);
                        return Some(out);
                    }
                    out.entries.push(PeExport { ordinal, name: Some(name), rva, forwarder: forwarder.clone() });
                }
            }
            None => {
                if !budget.take(forwarder.as_ref().map_or(0, String::len)) {
                    budget.finish(&mut out.truncated, &mut out.warnings);
                    return Some(out);
                }
                out.entries.push(PeExport { ordinal, name: None, rva, forwarder });
            }
        }
    }
    Some(out)
}

/// Add PE facts: imports with IAT slots, mitigations, unwind-table function starts, TLS
/// callbacks, subsystem and data directories.
pub fn enrich(data: &[u8], b: &mut Binary) {
    let h = match headers(data) {
        Ok(h) => h,
        Err(e) => {
            b.warnings.push(format!("PE headers: {e}"));
            return;
        }
    };
    b.image_base = h.image_base;
    b.entry = if h.entry_rva != 0 { h.image_va(u64::from(h.entry_rva)) } else { None };
    if h.entry_rva != 0 && b.entry.is_none() {
        b.warnings.push("the PE entry point exceeds the image address space".into());
    }
    if h.characteristics & 0x2000 != 0 {
        b.kind = "shared-library";
    }
    let r = Reader::new(data);
    // Imports (ours carry IAT slots and ordinals).
    let mut imps = ImportTable::default();
    let mut import_budget = TableTextBudget::new(DEFAULT_MAX_TABLE_TEXT_BYTES);
    normal_imports(data, &h, &mut imps, &mut import_budget);
    if !imps.rows.is_empty() {
        b.imports.clear();
        let mut libs = BTreeSet::new();
        for i in &imps.rows {
            let name = i.name.clone().unwrap_or_else(|| format!("{}#{}", i.library.trim_end_matches(".dll").trim_end_matches(".DLL"), i.ordinal.unwrap_or(0)));
            libs.insert(i.library.clone());
            if let Some(slot) = i.slot_va {
                b.import_slots.push(ImportSlot { addr: slot, name: name.clone(), library: Some(i.library.clone()), kind: "slot" });
            }
            b.imports.push(Import { name, library: Some(i.library.clone()), ordinal: i.ordinal, slot: i.slot_va });
        }
        b.libraries = libs.into_iter().collect();
    }
    b.warnings.extend(imps.warnings);
    // Delay-loaded imports: calls go through their own slots, so they get names too.
    let mut delayed = ImportTable::default();
    delay_imports(data, &h, &mut delayed, &mut import_budget);
    for i in delayed.rows {
        let name = i.name.clone().unwrap_or_else(|| format!("{}#{}", i.library.trim_end_matches(".dll").trim_end_matches(".DLL"), i.ordinal.unwrap_or(0)));
        if !b.libraries.contains(&i.library) {
            b.libraries.push(i.library.clone());
        }
        if let Some(slot) = i.slot_va {
            b.import_slots.push(ImportSlot { addr: slot, name: name.clone(), library: Some(i.library.clone()), kind: "slot" });
        }
        b.imports.push(Import { name, library: Some(i.library), ordinal: i.ordinal, slot: i.slot_va });
    }
    import_budget.finish(&mut delayed.truncated, &mut delayed.warnings);
    b.warnings.extend(delayed.warnings);
    // Mitigations.
    let dc = h.dll_characteristics;
    b.mitigations.pie = Some(dc & 0x40 != 0);
    b.mitigations.nx = Some(dc & 0x100 != 0);
    b.mitigations.high_entropy_va = Some(dc & 0x20 != 0);
    b.mitigations.control_flow_guard = Some(dc & 0x4000 != 0);
    b.mitigations.no_seh = Some(dc & 0x400 != 0);
    // Exception table → function starts (x64 and ARM64).
    if let Some((rva, size)) = h.dir(3)
        && let Some(off) = h.rva_to_offset(rva)
    {
        let ent = if h.machine == 0xaa64 { 8 } else { 12 };
        let n = (size as usize / ent).min(MAX_UNWIND);
        for i in 0..n {
            let Some(begin) = r.u32_le(off + i * ent) else { break };
            if begin != 0
                && let Some(address) = h.image_va(u64::from(begin))
            {
                b.function_starts.push(address);
            }
        }
    }
    // TLS callbacks run before the entry point.
    if let Some((rva, _)) = h.dir(9)
        && let Some(off) = h.rva_to_offset(rva)
    {
        let cb_va = if h.pe32_plus { r.u64_le(off + 24) } else { r.u32_le(off + 12).map(u64::from) };
        if let Some(cb_va) = cb_va.filter(|v| *v >= h.image_base) {
            let mut list = Vec::new();
            for i in 0..64u64 {
                let rva = cb_va - h.image_base + i * if h.pe32_plus { 8 } else { 4 };
                let Some(o) = u32::try_from(rva).ok().and_then(|r| h.rva_to_offset(r)) else { break };
                let v = if h.pe32_plus { r.u64_le(o) } else { r.u32_le(o).map(u64::from) };
                match v {
                    Some(v) if v != 0 => list.push(v),
                    _ => break,
                }
            }
            if !list.is_empty() {
                b.info.insert("tls_callbacks".into(), list.iter().map(|v| format!("{v:#x}")).collect::<Vec<_>>().join(", "));
                b.function_starts.extend(list);
            }
        }
    }
    if h.dir(14).is_some() {
        b.info.insert("dotnet".into(), "CLR header present (.NET assembly)".into());
    }
    b.info.insert("subsystem".into(), subsystem_name(h.subsystem).into());
    b.info.insert("timestamp".into(), h.timestamp.to_string());
    b.info.insert("linker_version".into(), format!("{}.{}", h.linker.0, h.linker.1));
    let rich = rich_header(data, h.pe_offset);
    if rich.is_some() {
        b.info.insert("rich_header".into(), "present (built with Microsoft tools)".into());
    }
    let dirs: Vec<_> = h
        .data_dirs
        .iter()
        .enumerate()
        .filter(|(_, (rva, size))| *rva != 0 || *size != 0)
        .map(|(i, (rva, size))| json!({"name": DIR_NAMES.get(i).copied().unwrap_or("?"), "rva": format!("{rva:#x}"), "size": size}))
        .collect();
    let secs: Vec<_> = h.sections.iter().map(|s| json!({"name": s.name, "characteristics": format!("{:#x}", s.characteristics)})).collect();
    b.details = json!({
        "machine": format!("{:#x}", h.machine),
        "pe32_plus": h.pe32_plus,
        "characteristics": format!("{:#x}", h.characteristics),
        "dll_characteristics": format!("{dc:#x}"),
        "subsystem": subsystem_name(h.subsystem),
        "data_directories": dirs,
        "sections": secs,
        "rich_header": rich.map(|v| v.into_iter().map(|(p, bld, c)| json!({"product": p, "build": bld, "count": c})).collect::<Vec<_>>()),
    });
}

// ---------------------------------------------------------------------------------------------
// Resources

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceEntry {
    pub type_name: String,
    pub name: String,
    pub language: u32,
    #[serde(with = "hex_addr")]
    pub rva: u64,
    pub size: u32,
    pub file_offset: Option<u64>,
    pub codepage: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IconEntry {
    pub width: u32,
    pub height: u32,
    pub bit_count: u16,
    pub bytes: u32,
    pub icon_id: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct IconGroup {
    pub name: String,
    pub icons: Vec<IconEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct VersionInfo {
    pub file_version: Option<String>,
    pub product_version: Option<String>,
    pub strings: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Resources {
    pub entries: Vec<ResourceEntry>,
    pub truncated: bool,
    pub icon_groups: Vec<IconGroup>,
    pub version: Option<VersionInfo>,
    pub manifest: Option<String>,
    pub warnings: Vec<String>,
}

pub fn resource_type_name(id: u32) -> String {
    match id {
        1 => "RT_CURSOR",
        2 => "RT_BITMAP",
        3 => "RT_ICON",
        4 => "RT_MENU",
        5 => "RT_DIALOG",
        6 => "RT_STRING",
        7 => "RT_FONTDIR",
        8 => "RT_FONT",
        9 => "RT_ACCELERATOR",
        10 => "RT_RCDATA",
        11 => "RT_MESSAGETABLE",
        12 => "RT_GROUP_CURSOR",
        14 => "RT_GROUP_ICON",
        16 => "RT_VERSION",
        17 => "RT_DLGINCLUDE",
        19 => "RT_PLUGPLAY",
        20 => "RT_VXD",
        21 => "RT_ANICURSOR",
        22 => "RT_ANIICON",
        23 => "RT_HTML",
        24 => "RT_MANIFEST",
        _ => return format!("#{id}"),
    }
    .into()
}

fn utf16_at(r: &Reader<'_>, off: usize, units: usize) -> Option<String> {
    let mut v = Vec::with_capacity(units.min(1024));
    for i in 0..units.min(4096) {
        v.push(r.u16_le(off + i * 2)?);
    }
    Some(String::from_utf16_lossy(&v))
}

/// Walk the resource directory (type → name → language), bounded by `max_entries`.
pub fn parse_resources(data: &[u8], max_entries: usize) -> Result<Resources, FormatError> {
    let h = headers(data)?;
    let mut res = Resources::default();
    let Some((rva, size)) = h.dir(2) else { return Ok(res) };
    let base = h.rva_to_offset(rva).ok_or_else(|| FormatError::Malformed("resource directory is not in the file".into()))?;
    let available = data.get(base..).ok_or_else(|| FormatError::Truncated("resource directory".into()))?;
    let mapped_len = h.rva_file_len(rva).ok_or_else(|| FormatError::Malformed("resource directory is not in the file".into()))?;
    if size as usize > available.len() {
        res.warnings.push("resource directory runs past the end of the file; reading the available bytes".into());
    }
    if size as usize > mapped_len {
        res.warnings.push("resource directory runs past its mapped extent; reading the available bytes".into());
    }
    let r = Reader::new(available.get(..(size as usize).min(mapped_len).min(available.len())).unwrap_or(&[]));
    let mut visited = BTreeSet::new();
    // (dir offset relative to base, depth, path)
    type Path = Vec<(Option<u32>, String)>;
    let mut stack: Vec<(usize, u8, Path)> = vec![(0, 0, Vec::new())];
    while let Some((dir, depth, path)) = stack.pop() {
        if !visited.insert(dir) || depth > 3 {
            res.warnings.push("resource directory loops or nests too deep; skipped a branch".into());
            continue;
        }
        let at = dir;
        let (Some(named), Some(ids)) = (r.u16_le(at + 12), r.u16_le(at + 14)) else {
            res.warnings.push("resource directory cut short".into());
            continue;
        };
        let n = usize::from(named) + usize::from(ids);
        let complete = r.len().saturating_sub(at + 16) / 8;
        if n > complete {
            res.warnings.push("resource directory entries cut short".into());
        }
        for i in (0..n.min(complete)).rev() {
            let e = at + 16 + i * 8;
            let (Some(name), Some(target)) = (r.u32_le(e), r.u32_le(e + 4)) else { break };
            let label = if name & 0x8000_0000 != 0 {
                let so = (name & 0x7fff_ffff) as usize;
                let Some(label) = r.u16_le(so).and_then(|len| {
                    let start = so.checked_add(2)?;
                    let units = usize::from(len);
                    r.slice(start, units.checked_mul(2)?)?;
                    utf16_at(&r, start, units)
                }) else {
                    res.warnings.push("resource name runs past the directory; skipped an entry".into());
                    continue;
                };
                (None, label)
            } else {
                (Some(name), name.to_string())
            };
            let mut p = path.clone();
            p.push(label);
            if target & 0x8000_0000 != 0 {
                stack.push(((target & 0x7fff_ffff) as usize, depth + 1, p));
            } else {
                if res.entries.len() >= max_entries {
                    res.truncated = true;
                    continue;
                }
                let de = target as usize;
                if r.slice(de, 16).is_none() {
                    res.warnings.push("resource data entry runs past the directory; skipped an entry".into());
                    continue;
                }
                let (Some(drva), Some(dsize)) = (r.u32_le(de), r.u32_le(de + 4)) else { continue };
                let (type_id, type_label) = p.first().cloned().unwrap_or((None, String::new()));
                let type_name = match type_id {
                    Some(id) => resource_type_name(id),
                    None => type_label,
                };
                let name = p.get(1).map(|x| x.1.clone()).unwrap_or_default();
                let language = p.get(2).and_then(|x| x.0).unwrap_or(0);
                res.entries.push(ResourceEntry {
                    type_name,
                    name,
                    language,
                    rva: u64::from(drva),
                    size: dsize,
                    file_offset: h.rva_to_offset(drva).map(|o| o as u64),
                    codepage: r.u32_le(de + 8).unwrap_or(0),
                });
            }
        }
    }
    res.entries.sort_by(|a, b| (a.type_name.as_str(), a.name.as_str(), a.language).cmp(&(b.type_name.as_str(), b.name.as_str(), b.language)));
    // Decode the interesting ones.
    for e in &res.entries {
        let Some(off) = e.file_offset.and_then(|o| usize::try_from(o).ok()) else { continue };
        let Some(bytes) = data.get(off..off.saturating_add(e.size as usize).min(data.len())) else { continue };
        match e.type_name.as_str() {
            "RT_GROUP_ICON" => {
                let g = Reader::new(bytes);
                let count = usize::from(g.u16_le(4).unwrap_or(0)).min(256);
                let icons = (0..count)
                    .filter_map(|i| {
                        let o = 6 + i * 14;
                        let w = g.u8(o)?;
                        let hgt = g.u8(o + 1)?;
                        Some(IconEntry {
                            width: if w == 0 { 256 } else { u32::from(w) },
                            height: if hgt == 0 { 256 } else { u32::from(hgt) },
                            bit_count: g.u16_le(o + 6)?,
                            bytes: g.u32_le(o + 8)?,
                            icon_id: g.u16_le(o + 12)?,
                        })
                    })
                    .collect();
                res.icon_groups.push(IconGroup { name: e.name.clone(), icons });
            }
            "RT_VERSION" if res.version.is_none() => res.version = parse_version_info(bytes),
            "RT_MANIFEST" if res.manifest.is_none() => {
                let n = bytes.len().min(64 * 1024);
                res.manifest = Some(String::from_utf8_lossy(bytes.get(..n).unwrap_or(&[])).into_owned());
            }
            _ => {}
        }
    }
    Ok(res)
}

/// Decode a `VS_VERSIONINFO` block: fixed file/product versions and the string tables.
pub fn parse_version_info(bytes: &[u8]) -> Option<VersionInfo> {
    let r = Reader::new(bytes);
    let mut info = VersionInfo::default();
    walk_version(&r, 0, bytes.len(), 0, &mut info);
    if info.file_version.is_none() && info.strings.is_empty() { None } else { Some(info) }
}

fn align4(x: usize) -> usize {
    x.saturating_add(3) & !3
}

/// The offset of a child version node after `value_bytes` of value, when one is present rather
/// than only alignment padding.
fn version_child_start(r: &Reader<'_>, value_off: usize, value_bytes: usize, node_end: usize) -> Option<usize> {
    let start = align4(value_off.saturating_add(value_bytes));
    let len = r.u16_le(start)?;
    (len >= 6 && start.checked_add(usize::from(len)).is_some_and(|end| end <= node_end)).then_some(start)
}

/// Read the version nodes between `start` and `end` into `info`: the fixed file and product
/// versions, then each string. `depth` counts levels below VS_VERSION_INFO, so strings are at 3.
fn walk_version(r: &Reader<'_>, start: usize, end: usize, depth: u8, info: &mut VersionInfo) {
    if depth > 6 {
        return;
    }
    let mut pos = start;
    while pos + 6 <= end && info.strings.len() < 512 {
        let Some(len) = r.u16_le(pos).map(usize::from) else { return };
        if len < 6 {
            return;
        }
        let node_end = pos.saturating_add(len).min(end);
        let value_len = usize::from(r.u16_le(pos + 2).unwrap_or(0));
        let is_text = r.u16_le(pos + 4) == Some(1);
        // key
        let mut k = pos + 6;
        let mut key = Vec::new();
        while k + 2 <= node_end && key.len() < 256 {
            let Some(u) = r.u16_le(k) else { break };
            k += 2;
            if u == 0 {
                break;
            }
            key.push(u);
        }
        let key = String::from_utf16_lossy(&key);
        let value_off = align4(k);
        let value_bytes = if is_text { value_len * 2 } else { value_len };
        match key.as_str() {
            "VS_VERSION_INFO" => {
                // The fixed record is 13 DWORDs. Adjacent children or bytes after this node
                // cannot supply fields missing from its declared binary value.
                let fixed = r
                    .slice(value_off, value_len)
                    .filter(|_| r.u16_le(pos + 4) == Some(0) && value_len >= 52 && value_off.checked_add(value_len).is_some_and(|end| end <= node_end))
                    .map(Reader::new);
                if let Some(fixed) = fixed.filter(|fixed| fixed.u32_le(0) == Some(0xfeef_04bd)) {
                    let v = |ms: Option<u32>, ls: Option<u32>| -> Option<String> {
                        let (ms, ls) = (ms?, ls?);
                        Some(format!("{}.{}.{}.{}", ms >> 16, ms & 0xffff, ls >> 16, ls & 0xffff))
                    };
                    info.file_version = v(fixed.u32_le(8), fixed.u32_le(12));
                    info.product_version = v(fixed.u32_le(16), fixed.u32_le(20));
                }
                walk_version(r, align4(value_off + value_bytes), node_end, depth + 1, info);
            }
            "StringFileInfo" => walk_version(r, align4(value_off + value_bytes), node_end, depth + 1, info),
            "VarFileInfo" => {}
            // A string with an empty value stores no value at all (wValueLength 0), as
            // OleSelfRegister does. Strings sit right under a string table, one level below it;
            // a leaf deeper down is inside another node's value. A value read earlier is kept.
            _ if depth == 3 && !key.is_empty() && is_text && value_len == 0 && value_off >= node_end => {
                info.strings.entry(key).or_default();
            }
            _ if depth >= 2 && is_text && value_len > 0 => {
                let units = value_len.min((node_end.saturating_sub(value_off)) / 2);
                let v = utf16_at(r, value_off, units).unwrap_or_default();
                info.strings.insert(key, v.trim_end_matches('\0').to_string());
            }
            // Some linkers write a string with wType 0; its value is still UTF-16LE. A child
            // node after the value means this is a container, not a string.
            _ if depth >= 2 && !is_text && !key.is_empty() && value_len > 0 => {
                if let Some(child) = version_child_start(r, value_off, value_len, node_end) {
                    walk_version(r, child, node_end, depth + 1, info);
                } else if depth == 3 {
                    let units = (value_len / 2).min((node_end.saturating_sub(value_off)) / 2);
                    let v = utf16_at(r, value_off, units).unwrap_or_default();
                    info.strings.insert(key, v.trim_end_matches('\0').to_string());
                }
            }
            _ => walk_version(r, align4(value_off + value_bytes), node_end, depth + 1, info),
        }
        let next = align4(pos + len);
        if next <= pos {
            return;
        }
        pos = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_rvas_stop_before_file_padding() {
        let mut pe = craft_anything_testkit::pe64();
        let first = headers(&pe).unwrap().sections[0].raw_offset;
        let opt = Reader::new(&pe).u32_le(0x3c).unwrap() as usize + 24;
        let size_headers = 0x200_u32;
        pe[opt + 60..opt + 64].copy_from_slice(&size_headers.to_le_bytes());
        let coff = opt - 20;
        let section = opt + usize::from(Reader::new(&pe).u16_le(coff + 16).unwrap());
        pe[section + 20..section + 24].copy_from_slice(&(first + 0x400).to_le_bytes());
        let h = headers(&pe).unwrap();
        assert_eq!(h.rva_to_offset(size_headers - 1), Some(0x1ff));
        assert_eq!(h.rva_to_offset(size_headers), None);
        assert_eq!(h.rva_to_offset(h.sections[0].rva), Some((first + 0x400) as usize));
    }

    #[test]
    fn imports_carry_iat_slots() {
        let pe = craft_anything_testkit::pe64();
        let h = headers(&pe).unwrap();
        let imps = imports(&pe, &h);
        assert_eq!(imps.len(), 1);
        assert_eq!(imps[0].name.as_deref(), Some("puts"));
        assert_eq!(imps[0].library, "msvcrt.dll");
        assert_eq!(imps[0].slot_va, craft_anything_testkit::PE_BASE + craft_anything_testkit::PE_IAT_RVA);
    }

    #[test]
    fn resources_icons_and_version() {
        let pe = craft_anything_testkit::pe64();
        let res = parse_resources(&pe, 100).unwrap();
        assert_eq!(res.entries.len(), 3, "{:?}", res.entries);
        assert_eq!(res.icon_groups.len(), 1);
        assert_eq!(res.icon_groups[0].icons[0].width, 16);
        let v = res.version.unwrap();
        assert_eq!(v.file_version.as_deref(), Some("1.2.3.4"));
        assert_eq!(v.strings.get("CompanyName").map(String::as_str), Some("Craft Test"));
        assert_eq!(v.strings.get("ProductName").map(String::as_str), Some("Craft Sample"));
        let capped = parse_resources(&pe, 1).unwrap();
        assert!(capped.truncated);
    }

    #[test]
    fn damaged_pes_never_panic() {
        let pe = craft_anything_testkit::pe64();
        for m in craft_anything_testkit::mutations(&pe) {
            let _ = parse_resources(&m, 1000);
            if let Ok(h) = headers(&m) {
                let _ = imports(&m, &h);
            }
            let _ = crate::parse_binary(&m);
        }
    }
}

#[cfg(test)]
mod shared_parity {
    use super::*;
    fn compare(data: &[u8]) {
        assert_eq!(legacy::has_pe_header(data), has_pe_header(data));
        match (legacy::headers(data), headers(data)) {
            (Ok(a), Ok(b)) => {
                assert_eq!(format!("{a:?}"), format!("{b:?}"));
                assert_eq!(serde_json::to_value(&a.sections).unwrap(), serde_json::to_value(&b.sections).unwrap());
                for rva in [0, 1, 127, 4095, 4096, 4097, u32::MAX] {
                    assert_eq!(a.rva_to_offset(rva), b.rva_to_offset(rva));
                    assert_eq!(a.rva_file_len(rva), b.rva_file_len(rva));
                    assert_eq!(a.image_va(u64::from(rva)), b.image_va(u64::from(rva)));
                }
                assert_eq!(a.image_va(u64::MAX), b.image_va(u64::MAX));
                for i in 0..18 {
                    assert_eq!(a.dir(i), b.dir(i));
                }
            }
            (Err(a), Err(b)) => assert_eq!(a, b),
            other => panic!("PE parity mismatch: {other:?}"),
        }
    }
    #[test]
    fn shared_headers_errors_bounds_and_mapping_match() {
        for wide in [false, true] {
            let mut data = vec![0; 8192];
            data[..2].copy_from_slice(b"MZ");
            data[0x3c..0x40].copy_from_slice(&128u32.to_le_bytes());
            data[128..132].copy_from_slice(b"PE\0\0");
            let opt = 152;
            let size = if wide { 240u16 } else { 224 };
            data[148..150].copy_from_slice(&size.to_le_bytes());
            data[opt..opt + 2].copy_from_slice(&if wide { 0x20bu16 } else { 0x10b }.to_le_bytes());
            data[opt + 60..opt + 64].copy_from_slice(&512u32.to_le_bytes());
            let section = opt + usize::from(size);
            for (index, rva, raw) in [(0, 4096u32, 512u32), (1, 4097, u32::MAX)] {
                let base = section + index * 40;
                data[base..base + 4].copy_from_slice(b"test");
                for (field, value) in [(8, 32u32), (12, rva), (16, 16), (20, raw)] {
                    data[base + field..base + field + 4].copy_from_slice(&value.to_le_bytes());
                }
            }
            for count in [0u16, 1, 96, 97, u16::MAX] {
                data[134..136].copy_from_slice(&count.to_le_bytes());
                compare(&data);
                if count == 97 {
                    assert_eq!(headers(&data).unwrap().sections.len(), 96);
                }
            }
            for len in 0..450 {
                compare(&data[..len]);
            }
            for pos in 0..500 {
                let old = data[pos];
                data[pos] = 255;
                compare(&data);
                data[pos] = old;
            }
        }
        for s in 0..256 {
            assert_eq!(legacy::subsystem_name(s), subsystem_name(s));
        }
    }
    #[test]
    fn shared_rich_header_keeps_bounded_entries() {
        let mut data = vec![0; 4096];
        data[64..68].copy_from_slice(&0x536e6144u32.to_le_bytes());
        data[3000..3004].copy_from_slice(b"Rich");
        assert_eq!(rich_header(&data, 4096).unwrap().len(), 256);
        for off in [0, 63, 64, 80, 3000, 3004, 3008, 4096, usize::MAX] {
            assert_eq!(legacy::rich_header(&data, off), rich_header(&data, off));
        }
        for len in (0..data.len()).step_by(13) {
            assert_eq!(legacy::rich_header(&data[..len], 4096), rich_header(&data[..len], 4096));
        }
    }
}
