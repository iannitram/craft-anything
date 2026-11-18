//! Go programs: the build information block (Go version, main module, dependencies, build
//! settings) and the function name table (pclntab) of ELF, PE and Mach-O files.
//!
//! The build information block starts with the magic `\xff Go buildinf:` on a 16-byte aligned
//! address. Since Go 1.18 it holds two length-prefixed strings inline; older toolchains store two
//! pointers to Go string headers instead, which we resolve through the file's address mappings.
//! The function table is found by section name or by scanning for its header, and gives every
//! compiled function's name and entry address.

use crate::bytes::Reader;
use crate::{FormatError, hex_addr};
use object::{Object, ObjectSection, ObjectSegment};
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

/// Largest input we parse (256 MiB).
pub const MAX_INPUT: usize = 256 << 20;
/// Longest string read from the build information (1 MiB).
pub const MAX_STRING: usize = 1 << 20;
/// Most functions read from the function table.
pub const MAX_FUNCTIONS: usize = 500_000;
/// Longest function name.
const MAX_NAME: usize = 4096;

const MAGIC: &[u8; 14] = b"\xff Go buildinf:";
const SENTINEL_START: [u8; 16] = [0x30, 0x77, 0xaf, 0x0c, 0x92, 0x74, 0x08, 0x02, 0x41, 0xe1, 0xc1, 0x07, 0xe6, 0xd6, 0x18, 0xe6];
const SENTINEL_END: [u8; 16] = [0xf9, 0x32, 0x43, 0x31, 0x86, 0x18, 0x20, 0x72, 0x00, 0x82, 0x42, 0x10, 0x41, 0x16, 0xd8, 0xf2];

/// A module line: `mod`, `dep`, or the `=>` replacement that follows one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoModule {
    pub path: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sum: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replace: Option<Box<GoModule>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoSetting {
    pub key: String,
    pub value: String,
}

/// The decoded build information block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoBuildInfo {
    /// File offset of the 32-byte header.
    pub offset: u64,
    #[serde(with = "hex_addr")]
    pub address: u64,
    pub pointer_size: u8,
    /// `inline` (Go 1.18+) or `pointer` (older toolchains).
    pub encoding: &'static str,
    pub big_endian: bool,
    pub go_version: Option<String>,
    /// The `path` line: the main package's import path.
    pub path: Option<String>,
    pub main: Option<GoModule>,
    pub deps: Vec<GoModule>,
    pub settings: Vec<GoSetting>,
    /// Lines we could not interpret (non-UTF-8 lines are given as `base64:…`).
    pub unparsed_lines: Vec<String>,
    /// True when the module text was present and every line was understood.
    pub complete: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoFunction {
    pub name: String,
    #[serde(with = "hex_addr")]
    pub entry: u64,
}

/// The function name table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoPclntab {
    /// Layout family: `1.2`, `1.16`, `1.18`, `1.20+`.
    pub layout: &'static str,
    pub offset: u64,
    /// `section` (found by name) or `scan` (found by its header).
    pub located_by: &'static str,
    pub section: Option<String>,
    pub pointer_size: u8,
    pub quantum: u8,
    pub big_endian: bool,
    /// Function count the header declares.
    pub declared_functions: u64,
    /// Where entry addresses are based: `header` (the table's own text start), `text-section`
    /// (the header field is zero until load-time relocation, so the text section address is used),
    /// or `absolute` (older layouts store full addresses).
    pub text_start_source: &'static str,
    pub functions: Vec<GoFunction>,
    /// Some declared functions were omitted because of unreadable records/names or a limit.
    pub truncated: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchRegion {
    pub name: String,
    pub offset: u64,
    pub size: u64,
    /// How the region was chosen.
    pub how: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoPackage {
    pub package: String,
    pub functions: usize,
}

/// Everything learned about a Go program.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GoReport {
    /// `elf`, `pe` or `macho`.
    pub container: &'static str,
    /// The universal-binary slice that was read, when the input was one.
    pub slice: Option<String>,
    pub slices_available: Vec<String>,
    pub architecture: String,
    pub is_64: bool,
    pub big_endian: bool,
    pub search_region: Option<SearchRegion>,
    pub mappings: usize,
    /// Address ranges that occupy memory but have no bytes in the file (zero-filled).
    pub zero_fills: usize,
    pub buildinfo: Option<GoBuildInfo>,
    pub pclntab: Option<GoPclntab>,
    /// Functions per package, largest first.
    pub packages: Vec<GoPackage>,
    /// True when either structure was found.
    pub is_go: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
struct Mapping {
    va: u64,
    file_off: u64,
    file_size: u64,
}

struct Mapped<'a> {
    data: &'a [u8],
    maps: Vec<Mapping>,
}

impl Mapped<'_> {
    /// Bytes at a virtual address, when they all come from the file.
    fn read(&self, va: u64, len: usize) -> Option<&[u8]> {
        let len64 = u64::try_from(len).ok()?;
        for m in &self.maps {
            if va >= m.va && va.checked_add(len64)? <= m.va.checked_add(m.file_size)? {
                let off = usize::try_from(m.file_off.checked_add(va - m.va)?).ok()?;
                return self.data.get(off..off.checked_add(len)?);
            }
        }
        None
    }

    fn va_of(&self, off: u64) -> Option<u64> {
        self.maps.iter().find(|m| off >= m.file_off && off < m.file_off.saturating_add(m.file_size)).and_then(|m| m.va.checked_add(off - m.file_off))
    }
}

fn uint(r: &Reader<'_>, off: usize, size: u8, big: bool) -> Option<u64> {
    match (size, big) {
        (4, false) => r.u32_le(off).map(u64::from),
        (4, true) => r.u32_be(off).map(u64::from),
        (8, false) => r.u64_le(off),
        (8, true) => r.u64_be(off),
        _ => None,
    }
}

/// The function names and entries in a Go program's function table, or none for other programs.
/// Cheap for them: only a file with Go's build-information marker or a `gopclntab` section is read.
pub fn functions(data: &[u8]) -> Vec<GoFunction> {
    let marked = |m: &[u8]| data.windows(m.len()).any(|w| w == m);
    if !marked(MAGIC) && !marked(b"gopclntab") {
        return Vec::new();
    }
    match inspect(data, None) {
        // shortcut: a universal binary's first slice may not be the one being disassembled, so its
        // names are left out rather than put on the wrong addresses; pass the slice through if needed.
        Ok(r) if r.slice.is_none() => r.pclntab.map(|p| p.functions).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Read a Go program. `architecture` picks a slice of a universal binary (the first otherwise).
pub fn inspect(data: &[u8], architecture: Option<&str>) -> Result<GoReport, FormatError> {
    if data.len() > MAX_INPUT {
        return Err(FormatError::TooLarge(format!("{} bytes; the limit is {MAX_INPUT}", data.len())));
    }
    let r = Reader::new(data);
    let mut slice = None;
    let mut slices_available = Vec::new();
    let mut body = data;
    if matches!(r.u32_be(0), Some(0xcafe_babe | 0xcafe_babf)) {
        let slices = crate::macho::fat_slices(data)?;
        slices_available = slices.iter().map(|s| s.arch.clone()).collect();
        let chosen = match architecture {
            Some(a) => slices
                .iter()
                .find(|s| s.arch.eq_ignore_ascii_case(a))
                .ok_or_else(|| FormatError::Unsupported(format!("no {a} slice; this universal binary has {}", slices_available.join(", "))))?,
            None => slices.first().ok_or_else(|| FormatError::Malformed("universal binary without slices".into()))?,
        };
        let start = usize::try_from(chosen.offset).map_err(|_| FormatError::Malformed("slice offset".into()))?;
        let len = usize::try_from(chosen.size).map_err(|_| FormatError::Malformed("slice size".into()))?;
        body = Reader::new(data).slice(start, len).ok_or_else(|| FormatError::Truncated("slice".into()))?;
        slice = Some(chosen.arch.clone());
    }
    let file = object::File::parse(body).map_err(|e| FormatError::Malformed(e.to_string()))?;
    let container = match file.format() {
        object::BinaryFormat::Elf => "elf",
        object::BinaryFormat::Pe => "pe",
        object::BinaryFormat::MachO => "macho",
        other => return Err(FormatError::Unsupported(format!("{other:?} files are not read by this tool; pass an ELF, PE or Mach-O program"))),
    };
    let big = !file.is_little_endian();
    let mut warnings = Vec::new();
    let mut maps = Vec::new();
    let mut zero_fills = 0usize;
    for seg in file.segments().take(4096) {
        let (fo, fs) = seg.file_range();
        let ms = seg.size();
        if fs < ms {
            zero_fills += 1;
        }
        if fs > 0 {
            maps.push(Mapping { va: seg.address(), file_off: fo, file_size: fs });
        }
    }
    // Sections fill gaps for files without segments (object files) and refine PE.
    if maps.is_empty() {
        for s in file.sections().take(4096) {
            if let Some((fo, fs)) = s.file_range() {
                maps.push(Mapping { va: s.address(), file_off: fo, file_size: fs });
            }
        }
    }
    let mapped = Mapped { data: body, maps };

    // Choose where to look for the build information.
    let region = search_region(&file, container, &mut warnings);
    let mut buildinfo = None;
    if let Some(reg) = &region {
        buildinfo = find_buildinfo(&mapped, reg, &mut warnings);
    }
    if buildinfo.is_none() {
        // Not where expected: look through the whole file before giving up.
        let whole = SearchRegion { name: "whole file".into(), offset: 0, size: body.len() as u64, how: "whole-file fallback" };
        buildinfo = find_buildinfo(&mapped, &whole, &mut warnings);
        if buildinfo.is_some() {
            warnings.push("the build information was found outside the usual region".into());
        }
    }
    let text_va = file.sections().find(|s| matches!(s.name().ok(), Some(".text" | "__text"))).map(|s| s.address());
    let pclntab = find_pclntab(&file, body, container, text_va, &mut warnings);
    let mut per_pkg: BTreeMap<String, usize> = BTreeMap::new();
    if let Some(p) = &pclntab {
        for f in &p.functions {
            *per_pkg.entry(package_of(&f.name)).or_default() += 1;
        }
    }
    let mut packages: Vec<GoPackage> = per_pkg.into_iter().map(|(package, functions)| GoPackage { package, functions }).collect();
    packages.sort_by(|a, b| b.functions.cmp(&a.functions).then_with(|| a.package.cmp(&b.package)));
    let is_go = buildinfo.is_some() || pclntab.is_some();
    Ok(GoReport {
        container,
        slice,
        slices_available,
        architecture: format!("{:?}", file.architecture()).to_ascii_lowercase(),
        is_64: file.is_64(),
        big_endian: big,
        search_region: region,
        mappings: mapped.maps.len(),
        zero_fills,
        buildinfo,
        pclntab,
        packages,
        is_go,
        warnings,
    })
}

fn search_region(file: &object::File<'_>, container: &str, warnings: &mut Vec<String>) -> Option<SearchRegion> {
    let named = |want: &str| file.sections().find(|s| s.name().ok() == Some(want));
    match container {
        "elf" => {
            if let Some(s) = named(".go.buildinfo") {
                if let object::SectionFlags::Elf { sh_flags } = s.flags()
                    && sh_flags & 0x800 != 0
                {
                    warnings.push(".go.buildinfo is compressed (SHF_COMPRESSED); compressed sections are not read".into());
                    return None;
                }
                let (offset, size) = s.file_range()?;
                return Some(SearchRegion { name: ".go.buildinfo".into(), offset, size, how: "section" });
            }
            file.segments().find_map(|seg| match seg.flags() {
                object::SegmentFlags::Elf { p_flags } if p_flags & 2 != 0 && p_flags & 1 == 0 => {
                    let (offset, size) = seg.file_range();
                    (size > 0).then(|| SearchRegion { name: "first writable, non-executable segment".into(), offset, size, how: "segment" })
                }
                _ => None,
            })
        }
        "pe" => file.sections().find_map(|s| match s.flags() {
            object::SectionFlags::Coff { characteristics: 0xc000_0040 } => {
                let (offset, size) = s.file_range()?;
                Some(SearchRegion { name: s.name().unwrap_or("?").to_string(), offset, size, how: "first initialised read-write data section" })
            }
            _ => None,
        }),
        _ => {
            if let Some(s) = named("__go_buildinfo") {
                let (offset, size) = s.file_range()?;
                return Some(SearchRegion { name: "__go_buildinfo".into(), offset, size, how: "section" });
            }
            file.segments().find_map(|seg| match seg.flags() {
                object::SegmentFlags::MachO { initprot, .. } if initprot & 3 == 3 && initprot & 4 == 0 => {
                    let (offset, size) = seg.file_range();
                    (size > 0).then(|| SearchRegion { name: seg.name().ok().flatten().unwrap_or("?").to_string(), offset, size, how: "read-write segment" })
                }
                _ => None,
            })
        }
    }
}

fn find_buildinfo(m: &Mapped<'_>, reg: &SearchRegion, warnings: &mut Vec<String>) -> Option<GoBuildInfo> {
    let start = usize::try_from(reg.offset).ok()?;
    let len = usize::try_from(reg.size).ok()?;
    let hay = Reader::new(m.data).slice(start, len.min(m.data.len().saturating_sub(start)))?;
    let mut i = 0usize;
    while let Some(p) = crate::find_bytes(hay.get(i..)?, MAGIC, usize::MAX) {
        let at = i.checked_add(p)?;
        let file_off = start.checked_add(at)? as u64;
        // The block sits on a 16-byte aligned address (fall back to file offset alignment).
        let aligned = match m.va_of(file_off) {
            Some(va) => va.is_multiple_of(16),
            None => file_off.is_multiple_of(16),
        };
        if aligned && let Some(b) = decode_buildinfo(m, start.checked_add(at)?, warnings) {
            return Some(b);
        }
        i = at.checked_add(1)?;
    }
    None
}

fn read_uvarint(r: &Reader<'_>, off: usize) -> Option<(u64, usize)> {
    r.uleb128(off)
}

fn decode_buildinfo(m: &Mapped<'_>, off: usize, warnings: &mut Vec<String>) -> Option<GoBuildInfo> {
    let r = Reader::new(m.data);
    let ptr_size = r.u8(off.checked_add(14)?)?;
    let flags = r.u8(off.checked_add(15)?)?;
    let big = flags & 1 != 0;
    let inline = flags & 2 != 0;
    let mut local_warnings = Vec::new();
    let (version, module) = if inline {
        let mut p = off.checked_add(32)?;
        let mut out = Vec::new();
        for _ in 0..2 {
            let (n, used) = read_uvarint(&r, p)?;
            let n = usize::try_from(n).ok()?;
            if n > MAX_STRING {
                local_warnings.push(format!("a build information string is {n} bytes; the limit is {MAX_STRING}"));
                return None;
            }
            p = p.checked_add(used)?;
            out.push(r.slice(p, n)?.to_vec());
            p = p.checked_add(n)?;
        }
        let module = out.pop()?;
        let version = out.pop()?;
        (version, module)
    } else {
        if ptr_size != 4 && ptr_size != 8 {
            return None;
        }
        let ps = usize::from(ptr_size);
        let v_ptr = uint(&r, off.checked_add(16)?, ptr_size, big)?;
        let m_ptr = uint(&r, off.checked_add(16)?.checked_add(ps)?, ptr_size, big)?;
        let read_go_string = |hdr: u64| -> Option<Vec<u8>> {
            let h = m.read(hdr, ps.checked_mul(2)?)?;
            let hr = Reader::new(h);
            let data = uint(&hr, 0, ptr_size, big)?;
            let len = usize::try_from(uint(&hr, ps, ptr_size, big)?).ok()?;
            if len > MAX_STRING {
                return None;
            }
            if len == 0 {
                return Some(Vec::new());
            }
            m.read(data, len).map(<[u8]>::to_vec)
        };
        let Some(version) = read_go_string(v_ptr) else {
            warnings.push("found a pointer-encoded build information header but could not follow its version pointer".into());
            return None;
        };
        let module = read_go_string(m_ptr).unwrap_or_else(|| {
            local_warnings.push("could not follow the module text pointer".into());
            Vec::new()
        });
        (version, module)
    };
    let go_version = String::from_utf8(version).ok().filter(|v| !v.is_empty());
    if go_version.as_deref().is_some_and(|v| !v.starts_with("go") && !v.starts_with("devel")) {
        local_warnings.push("the version string does not look like a Go release".into());
    }
    // Strip the 16-byte sentinels around the module text.
    let mut text = module.as_slice();
    if text.len() >= 33 && text.get(text.len() - 17) == Some(&b'\n') {
        if text.get(..16) != Some(&SENTINEL_START[..]) || text.get(text.len() - 16..) != Some(&SENTINEL_END[..]) {
            local_warnings.push("the module text sentinels are not the expected values".into());
        }
        text = text.get(16..text.len() - 16).unwrap_or(&[]);
    }
    let address = m.va_of(off as u64).unwrap_or(0);
    let mut b = GoBuildInfo {
        offset: off as u64,
        address,
        pointer_size: ptr_size,
        encoding: if inline { "inline" } else { "pointer" },
        big_endian: big,
        go_version,
        path: None,
        main: None,
        deps: Vec::new(),
        settings: Vec::new(),
        unparsed_lines: Vec::new(),
        complete: false,
        warnings: local_warnings,
    };
    parse_module_text(text, &mut b);
    b.complete = !text.is_empty() && b.unparsed_lines.is_empty() && b.warnings.is_empty();
    if text.is_empty() {
        b.warnings.push("no module text (built without module support, or stripped)".into());
    }
    Some(b)
}

/// Parse the `path`/`mod`/`dep`/`=>`/`build` lines.
fn parse_module_text(text: &[u8], b: &mut GoBuildInfo) {
    let mut last_was_dep: Option<bool> = None;
    for raw in text.split(|&c| c == b'\n').take(100_000) {
        if raw.is_empty() {
            continue;
        }
        let Ok(line) = std::str::from_utf8(raw) else {
            b.unparsed_lines.push(format!("base64:{}", base64(raw)));
            continue;
        };
        let mut parts = line.splitn(2, '\t');
        let kind = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        match kind {
            "path" => b.path = Some(rest.to_string()),
            "mod" | "dep" => match module_fields(rest) {
                Some(md) => {
                    if kind == "mod" {
                        b.main = Some(md);
                        last_was_dep = Some(false);
                    } else {
                        b.deps.push(md);
                        last_was_dep = Some(true);
                    }
                }
                None => b.unparsed_lines.push(line.to_string()),
            },
            "=>" => {
                let target = match last_was_dep {
                    Some(true) => b.deps.last_mut(),
                    Some(false) => b.main.as_mut(),
                    None => None,
                };
                match (module_fields_loose(rest), target) {
                    (Some(rep), Some(t)) if t.replace.is_none() => t.replace = Some(Box::new(rep)),
                    _ => b.unparsed_lines.push(line.to_string()),
                }
            }
            "build" => match build_setting(rest) {
                Some(s) => b.settings.push(s),
                None => b.unparsed_lines.push(line.to_string()),
            },
            _ => b.unparsed_lines.push(line.to_string()),
        }
    }
}

fn module_fields(rest: &str) -> Option<GoModule> {
    let f: Vec<&str> = rest.split('\t').collect();
    match f.as_slice() {
        [path, version] => Some(GoModule { path: path.to_string(), version: version.to_string(), sum: None, replace: None }),
        [path, version, sum] => {
            Some(GoModule { path: path.to_string(), version: version.to_string(), sum: (!sum.is_empty()).then(|| sum.to_string()), replace: None })
        }
        _ => None,
    }
}

/// Replacements may be local paths with no version.
fn module_fields_loose(rest: &str) -> Option<GoModule> {
    let f: Vec<&str> = rest.split('\t').collect();
    match f.as_slice() {
        [path] => Some(GoModule { path: path.to_string(), version: String::new(), sum: None, replace: None }),
        _ => module_fields(rest),
    }
}

fn build_setting(rest: &str) -> Option<GoSetting> {
    let (key, value_part) = if rest.starts_with('"') {
        let (k, used) = unquote_prefix(rest)?;
        (k, rest.get(used..)?.strip_prefix('=')?)
    } else {
        let eq = rest.find('=')?;
        (rest.get(..eq)?.to_string(), rest.get(eq + 1..)?)
    };
    if key.is_empty() {
        return None;
    }
    let value = if value_part.starts_with('"') {
        let (v, used) = unquote_prefix(value_part)?;
        if used != value_part.len() {
            return None;
        }
        v
    } else {
        value_part.to_string()
    };
    Some(GoSetting { key, value })
}

/// Decode a Go-quoted string at the start of `s`: (value, bytes consumed).
pub fn unquote_prefix(s: &str) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if bytes.first() != Some(&b'"') {
        return None;
    }
    let mut out: Vec<u8> = Vec::new();
    let mut i = 1usize;
    let hex = |b: &[u8]| -> Option<u32> {
        if !b.iter().all(u8::is_ascii_hexdigit) {
            return None;
        }
        u32::from_str_radix(std::str::from_utf8(b).ok()?, 16).ok()
    };
    while let Some(&c) = bytes.get(i) {
        match c {
            b'"' => return String::from_utf8(out).ok().map(|v| (v, i + 1)),
            b'\\' => {
                let e = *bytes.get(i + 1)?;
                i += 2;
                match e {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'a' => out.push(7),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'v' => out.push(11),
                    b'\\' => out.push(b'\\'),
                    b'"' => out.push(b'"'),
                    b'\'' => out.push(b'\''),
                    b'x' => {
                        out.push(u8::try_from(hex(bytes.get(i..i + 2)?)?).ok()?);
                        i += 2;
                    }
                    b'u' | b'U' => {
                        let n = if e == b'u' { 4 } else { 8 };
                        let ch = char::from_u32(hex(bytes.get(i..i + n)?)?)?;
                        let mut buf = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        i += n;
                    }
                    b'0'..=b'7' => {
                        let digits = std::str::from_utf8(bytes.get(i - 1..i + 2)?).ok()?;
                        out.push(u8::try_from(u32::from_str_radix(digits, 8).ok()?).ok()?);
                        i += 2;
                    }
                    _ => return None,
                }
            }
            b'\n' => return None,
            _ => {
                out.push(c);
                i += 1;
            }
        }
        if out.len() > MAX_STRING {
            return None;
        }
    }
    None
}

use craft_mime::transfer::base64;

/// The package part of a Go function name (`github.com/a/b.(*T).M` → `github.com/a/b`).
pub fn package_of(name: &str) -> String {
    // Import paths cannot contain '['; subsequent slashes may belong to type arguments.
    let prefix = name.split_once('[').map_or(name, |(prefix, _)| prefix);
    let slash = prefix.rfind('/').map(|i| i + 1).unwrap_or(0);
    let tail = prefix.get(slash..).unwrap_or("");
    match tail.find('.') {
        Some(dot) => name.get(..slash + dot).unwrap_or(name).to_string(),
        None => name.to_string(),
    }
}

// ---------------------------------------------------------------------------------------------
// pclntab

fn pcln_layout(magic: u32) -> Option<&'static str> {
    match magic {
        0xffff_fffb => Some("1.2"),
        0xffff_fffa => Some("1.16"),
        0xffff_fff0 => Some("1.18"),
        0xffff_fff1 => Some("1.20+"),
        _ => None,
    }
}

/// Header check at `off`: magic, two zero bytes, quantum 1/2/4, pointer size 4/8.
fn pcln_header(r: &Reader<'_>, off: usize) -> Option<(&'static str, bool, u8, u8)> {
    let (layout, big) = match (r.u32_le(off).and_then(pcln_layout), r.u32_be(off).and_then(pcln_layout)) {
        (Some(l), _) => (l, false),
        (None, Some(l)) => (l, true),
        _ => return None,
    };
    if r.u8(off + 4)? != 0 || r.u8(off + 5)? != 0 {
        return None;
    }
    let quantum = r.u8(off + 6)?;
    let ps = r.u8(off + 7)?;
    if !matches!(quantum, 1 | 2 | 4) || !matches!(ps, 4 | 8) {
        return None;
    }
    Some((layout, big, quantum, ps))
}

fn find_pclntab(file: &object::File<'_>, data: &[u8], container: &str, text_va: Option<u64>, warnings: &mut Vec<String>) -> Option<GoPclntab> {
    let r = Reader::new(data);
    let want = if container == "macho" { "__gopclntab" } else { ".gopclntab" };
    let mut named_section = None;
    let mut partial = None;
    if let Some(s) = file.sections().find(|s| s.name().ok() == Some(want))
        && let Some((fo, fs)) = s.file_range()
        && let Ok(off) = usize::try_from(fo)
    {
        let size = usize::try_from(fs).unwrap_or(usize::MAX);
        named_section = Some((off, off.saturating_add(size).min(data.len())));
        match read_pclntab(&r, off, size, "section", Some(want.to_string()), text_va) {
            Some(t) if t.declared_functions == 0 || !t.functions.is_empty() => return Some(t),
            Some(t) => partial = Some(t),
            None => warnings.push(format!("{want} is present but could not be read within its declared section")),
        }
    }
    // Index file-backed extents once; candidates advance monotonically through them.
    let mut ranges: Vec<_> = file
        .sections()
        .filter_map(|s| {
            let (off, size) = s.file_range()?;
            let start = usize::try_from(off).ok()?;
            let size = usize::try_from(size).unwrap_or(usize::MAX);
            let end = start.saturating_add(size).min(data.len());
            (start < end).then_some((start, end))
        })
        .collect();
    ranges.sort_unstable();
    let mut ranges = ranges.into_iter().peekable();
    let mut active_ends = BinaryHeap::new();
    // Scan for a header with plausible fields (PE files and stripped section tables).
    let mut i = 0usize;
    let mut tries = 0usize;
    while i.checked_add(8).is_some_and(|e| e <= data.len()) && tries < 4096 {
        let Some(hay) = data.get(i..) else { break };
        let pos = hay.windows(4).position(|w| matches!(w, [0xfb | 0xfa | 0xf0 | 0xf1, 0xff, 0xff, 0xff] | [0xff, 0xff, 0xff, 0xfb | 0xfa | 0xf0 | 0xf1]));
        let Some(pos) = pos else { break };
        let Some(at) = i.checked_add(pos) else { break };
        tries += 1;
        while ranges.peek().is_some_and(|&(start, _)| start <= at) {
            let Some((_, end)) = ranges.next() else { break };
            if end > at {
                active_ends.push(Reverse(end));
            }
        }
        while active_ends.peek().is_some_and(|&Reverse(end)| end <= at) {
            active_ends.pop();
        }
        // Overlapping sections all constrain a candidate; retain zero-size named bounds too.
        let end = active_ends.peek().map_or(data.len(), |&Reverse(end)| end);
        let end = named_section.filter(|&(start, end)| at == start || (at >= start && at < end)).map_or(end, |(_, named_end)| end.min(named_end));
        let size = end.saturating_sub(at);
        if pcln_header(&r, at).is_some()
            && let Some(t) = read_pclntab(&r, at, size, "scan", None, text_va)
            && !t.functions.is_empty()
        {
            return Some(t);
        }
        let Some(next) = at.checked_add(1) else { break };
        i = next;
    }
    partial
}

fn read_pclntab(r: &Reader<'_>, base: usize, size: usize, located_by: &'static str, section: Option<String>, text_va: Option<u64>) -> Option<GoPclntab> {
    let end = base.checked_add(size)?.min(r.len());
    let r = Reader::new(r.slice(0, end)?);
    let (layout, big, quantum, ps) = pcln_header(&r, base)?;
    let psz = usize::from(ps);
    let word = |off: usize| -> Option<u64> { uint(&r, base.checked_add(off)?, ps, big) };
    let u32at = |off: usize| -> Option<u32> {
        let o = base.checked_add(off)?;
        if big { r.u32_be(o) } else { r.u32_le(o) }
    };
    let in_table = |abs: usize| abs < end;
    let mut warnings = Vec::new();
    let nfunc = word(8)?;
    if nfunc > 50_000_000 {
        return None;
    }
    let take = usize::try_from(nfunc).ok()?.min(MAX_FUNCTIONS);
    let mut functions = Vec::with_capacity(take.min(65_536));
    // A shared name offset must not multiply a small table into gigabytes of owned strings.
    let name_budget = (64usize << 20).max(end.saturating_sub(base).saturating_mul(3));
    let mut name_bytes_left = name_budget;
    let mut name_budget_exhausted = false;
    let mut text_start_source = "absolute";
    let name_at = |abs: usize| -> Option<String> {
        if !in_table(abs) {
            return None;
        }
        let bytes = r.slice(abs, (end - abs).min(MAX_NAME + 1))?;
        let length = bytes.iter().position(|&byte| byte == 0)?;
        let s = r.cstr(abs, length)?;
        (!s.is_empty()).then_some(s)
    };
    match layout {
        "1.2" => {
            let functab = 8usize.checked_add(psz)?;
            for i in 0..take {
                let record = (|| {
                    let e = functab.checked_add(i.checked_mul(2 * psz)?)?;
                    let entry = word(e)?;
                    let funcoff = usize::try_from(word(e.checked_add(psz)?)?).ok()?;
                    let nameoff = u32at(funcoff.checked_add(psz)?)?;
                    Some((entry, base.checked_add(usize::try_from(nameoff).ok()?)?))
                })();
                let Some((entry, name)) = record else {
                    if i == 0 {
                        return None;
                    }
                    warnings.push(format!("function {i}: unreadable record; the remaining functions were skipped"));
                    break;
                };
                match name_at(name) {
                    Some(name) => {
                        let Some(remaining) = name_bytes_left.checked_sub(name.len()) else {
                            name_budget_exhausted = true;
                            break;
                        };
                        name_bytes_left = remaining;
                        functions.push(GoFunction { name, entry });
                    }
                    None => warnings.push(format!("function {i}: unreadable name")),
                }
                if warnings.len() > 64 {
                    break;
                }
            }
        }
        _ => {
            let new = layout != "1.16";
            let (mut text_start, nameoff_field, pcln_field) = if new { (word(8 + 2 * psz)?, 8 + 3 * psz, 8 + 7 * psz) } else { (0, 8 + 2 * psz, 8 + 6 * psz) };
            if new {
                text_start_source = "header";
                // Some toolchains leave this field zero and fill it in at load time.
                if text_start == 0
                    && let Some(t) = text_va
                {
                    text_start = t;
                    text_start_source = "text-section";
                }
            }
            let funcname = usize::try_from(word(nameoff_field)?).ok()?;
            let functab = usize::try_from(word(pcln_field)?).ok()?;
            if base.checked_add(functab)? >= end || base.checked_add(funcname)? >= end {
                return None;
            }
            let stride = if new { 8 } else { 2 * psz };
            for i in 0..take {
                let record = (|| {
                    let e = functab.checked_add(i.checked_mul(stride)?)?;
                    let (entry, funcoff) = if new {
                        (text_start.checked_add(u64::from(u32at(e)?))?, usize::try_from(u32at(e.checked_add(4)?)?).ok()?)
                    } else {
                        (word(e)?, usize::try_from(word(e.checked_add(psz)?)?).ok()?)
                    };
                    let f = functab.checked_add(funcoff)?;
                    let nameoff = u32at(f.checked_add(if new { 4 } else { psz })?)?;
                    Some((entry, base.checked_add(funcname)?.checked_add(usize::try_from(nameoff).ok()?)?))
                })();
                let Some((entry, name)) = record else {
                    if i == 0 {
                        return None;
                    }
                    warnings.push(format!("function {i}: unreadable record; the remaining functions were skipped"));
                    break;
                };
                match name_at(name) {
                    Some(name) => {
                        let Some(remaining) = name_bytes_left.checked_sub(name.len()) else {
                            name_budget_exhausted = true;
                            break;
                        };
                        name_bytes_left = remaining;
                        functions.push(GoFunction { name, entry });
                    }
                    None => warnings.push(format!("function {i}: unreadable name")),
                }
                if warnings.len() > 64 {
                    break;
                }
            }
        }
    }
    if warnings.len() > 64 {
        warnings.truncate(64);
        warnings.push("stopped after too many unreadable entries".into());
    }
    if name_budget_exhausted {
        warnings.push(format!("function names reached their {name_budget}-byte budget; the remaining functions were skipped"));
    }
    Some(GoPclntab {
        layout,
        offset: base as u64,
        located_by,
        section,
        pointer_size: ps,
        quantum,
        big_endian: big,
        declared_functions: nfunc,
        text_start_source,
        truncated: (functions.len() as u64) < nfunc,
        functions,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_testkit::formats as fx;

    fn shared_name_table(layout: u8, count: usize, name: &[u8]) -> (Vec<u8>, usize, usize) {
        let new = layout >= 18;
        let header = if layout == 2 {
            16
        } else if new {
            72
        } else {
            64
        };
        let stride = if new { 8 } else { 16 };
        let record_size = if new { 40 } else { 48 };
        let records = header + (count + 1) * stride;
        let names = records + count * record_size;
        let mut bytes = vec![0; names + name.len() + 1];
        let magic = match layout {
            2 => 0xfb,
            16 => 0xfa,
            18 => 0xf0,
            _ => 0xf1,
        };
        bytes[..8].copy_from_slice(&[magic, 0xff, 0xff, 0xff, 0, 0, 1, 8]);
        let put = |bytes: &mut [u8], at: usize, value: usize, width: usize| bytes[at..at + width].copy_from_slice(&(value as u64).to_le_bytes()[..width]);
        put(&mut bytes, 8, count, 8);
        if layout != 2 {
            put(&mut bytes, if new { 32 } else { 24 }, names, 8);
            put(&mut bytes, if new { 64 } else { 56 }, header, 8);
            if new {
                put(&mut bytes, 24, 0x1000, 8);
            }
        }
        let width = if new { 4 } else { 8 };
        for i in 0..count {
            let record = records + i * record_size;
            let entry = if new { i } else { 0x1000 + i };
            put(&mut bytes, header + i * stride, entry, width);
            put(&mut bytes, header + i * stride + width, if layout == 2 { record } else { record - header }, width);
            put(&mut bytes, record, entry, width);
            put(&mut bytes, record + width, if layout == 2 { names } else { 0 }, 4);
        }
        put(&mut bytes, header + count * stride, if new { count } else { 0x1000 + count }, width);
        bytes[names..names + name.len()].copy_from_slice(name);
        (bytes, records, record_size)
    }

    /// Every supported layout may point many function records at one long, terminated name.
    #[test]
    fn shared_function_names_have_a_retained_byte_budget_in_every_layout() {
        let name = vec![b'n'; 4096];
        for layout in [2, 16, 18, 20] {
            let (bytes, _, _) = shared_name_table(layout, 20_000, &name);
            assert!(bytes.len() < 2 << 20);
            let table = read_pclntab(&Reader::new(&bytes), 0, bytes.len(), "section", None, None).unwrap();
            assert!(table.functions.iter().map(|f| f.name.len()).sum::<usize>() <= 64 << 20);
            assert_eq!(table.declared_functions, 20_000);
            assert!(!table.functions.is_empty());
            assert!(table.truncated);
            assert!(table.warnings.iter().any(|w| w.contains("byte budget")));
            assert!(table.functions.iter().enumerate().all(|(i, f)| f.entry == 0x1000 + i as u64 && f.name.as_bytes() == name));
        }
    }

    /// Charge the decoded UTF-8 bytes, including replacement characters from malformed names.
    #[test]
    fn lossy_function_names_spend_their_expanded_byte_length() {
        let (bytes, _, _) = shared_name_table(20, 6000, &[0xff; 4096]);
        let table = read_pclntab(&Reader::new(&bytes), 0, bytes.len(), "section", None, None).unwrap();
        assert!(table.functions.iter().map(|f| f.name.len()).sum::<usize>() <= 64 << 20);
        assert!(table.truncated);
        assert_eq!(table.functions[0].name.chars().count(), 4096);
        assert!(table.functions[0].name.chars().all(|c| c == char::REPLACEMENT_CHARACTER));
    }

    /// A memory limit is distinct from the existing cap on unreadable-entry warnings.
    #[test]
    fn name_budget_diagnostic_survives_existing_unreadable_names() {
        let (mut bytes, records, size) = shared_name_table(20, 20_000, &[b'n'; 4096]);
        for i in 0..64 {
            bytes[records + i * size + 4..records + i * size + 8].fill(0xff);
        }
        let table = read_pclntab(&Reader::new(&bytes), 0, bytes.len(), "section", None, None).unwrap();
        assert_eq!(table.functions[0].entry, 0x1040);
        assert!(table.truncated);
        assert!(table.warnings.iter().any(|w| w.contains("byte budget")));
        assert!(!table.warnings.iter().any(|w| w.contains("too many unreadable")));
    }

    /// Repeated names are valid metadata and remain complete when their actual copies fit.
    #[test]
    fn ordinary_shared_function_names_stay_complete() {
        for layout in [2, 16, 18, 20] {
            let (bytes, _, _) = shared_name_table(layout, 100, b"main.shared");
            let table = read_pclntab(&Reader::new(&bytes), 0, bytes.len(), "section", None, None).unwrap();
            assert_eq!(table.functions.len(), 100);
            assert!(!table.truncated);
            assert!(table.warnings.is_empty());
            assert!(table.functions.iter().enumerate().all(|(i, f)| f.name == "main.shared" && f.entry == 0x1000 + i as u64));
        }
    }

    #[test]
    fn instantiated_functions_keep_the_declaring_package() {
        assert_eq!(package_of("example.org/app.F[example.net/lib.Type]"), "example.org/app");
        assert_eq!(package_of("net/http.(*pool[example.net/lib.T]).get"), "net/http");
        assert_eq!(package_of("example.org/app.Run"), "example.org/app");
    }

    fn legacy_prefix_table() -> Vec<u8> {
        let mut data = vec![0; 128];
        data[..8].copy_from_slice(&[0xfb, 0xff, 0xff, 0xff, 0, 0, 1, 8]);
        for (at, value) in [(8, 2u64), (16, 0x1000), (24, 88), (32, 0x1010), (40, 112)] {
            data[at..at + 8].copy_from_slice(&value.to_le_bytes());
        }
        data[96..100].copy_from_slice(&56u32.to_le_bytes());
        data[120..124].copy_from_slice(&67u32.to_le_bytes());
        data[56..67].copy_from_slice(b"main.first\0");
        data[67..79].copy_from_slice(b"main.second\0");
        data
    }

    fn prefix_tables() -> Vec<(Vec<u8>, usize, usize, usize)> {
        let funcs = [("main.first", 0), ("main.second", 0x10)];
        let old = fx::pclntab_116(0x1000, &funcs);
        let new = fx::pclntab_120(0x1000, &funcs);
        let old_start = Reader::new(&old).u64_le(56).unwrap() as usize;
        let new_start = Reader::new(&new).u64_le(64).unwrap() as usize;
        vec![(legacy_prefix_table(), 16, 16, 8), (old, old_start, 16, 8), (new, new_start, 8, 4)]
    }

    #[test]
    fn pclntab_keeps_functions_before_an_unreadable_record() {
        for (mut data, start, stride, width) in prefix_tables() {
            let complete = read_pclntab(&Reader::new(&data), 0, data.len(), "section", None, None).unwrap();
            assert_eq!(complete.functions.len(), 2);
            assert!(!complete.truncated);
            data[start + stride + width..start + 2 * stride].fill(0xff);
            let partial = read_pclntab(&Reader::new(&data), 0, data.len(), "section", None, None).unwrap();
            assert_eq!(partial.functions, complete.functions[..1]);
            assert_eq!(partial.declared_functions, 2);
            assert!(partial.truncated);
            assert!(partial.warnings.iter().any(|warning| warning.contains("function 1")));
        }
    }

    #[test]
    fn pclntab_keeps_functions_before_a_truncated_function_record() {
        for (data, start, stride, width) in prefix_tables() {
            let r = Reader::new(&data);
            let record =
                if width == 4 { r.u32_le(start + stride + width).unwrap() as usize + start } else { r.u64_le(start + stride + width).unwrap() as usize };
            let record = if r.u32_le(0) == Some(0xffff_fffa) { start + record } else { record };
            let partial = read_pclntab(&r, 0, record + width + 2, "section", None, None).unwrap();
            assert_eq!(partial.functions, [GoFunction { name: "main.first".into(), entry: 0x1000 }]);
            assert!(partial.truncated);
        }
    }

    #[test]
    fn pclntab_reports_incomplete_coverage_for_unreadable_names() {
        for count in [2, 70] {
            let mut data = fx::pclntab_120(0x1000, &vec![("main.first", 0); count]);
            let start = Reader::new(&data).u64_le(64).unwrap() as usize;
            for i in 1..count {
                let record = start + Reader::new(&data).u32_le(start + i * 8 + 4).unwrap() as usize;
                data[record + 4..record + 8].copy_from_slice(&u32::MAX.to_le_bytes());
            }
            let partial = read_pclntab(&Reader::new(&data), 0, data.len(), "section", None, None).unwrap();
            assert_eq!(partial.functions.len(), 1);
            assert!(partial.truncated);
            if count == 70 {
                assert!(partial.warnings.iter().any(|warning| warning.contains("stopped")));
            }
        }
    }

    #[test]
    fn pclntab_address_overflow_keeps_an_earlier_valid_function() {
        let data = fx::pclntab_120(u64::MAX - 4, &[("main.first", 0), ("main.second", 8)]);
        let partial = read_pclntab(&Reader::new(&data), 0, data.len(), "section", None, None).unwrap();
        assert_eq!(partial.functions, [GoFunction { name: "main.first".into(), entry: u64::MAX - 4 }]);
        assert!(partial.truncated);
    }

    #[test]
    fn go_inspection_exposes_a_recovered_function_prefix() {
        let mut data = fx::go_elf();
        let (_, start, _) = pclntab_section(&data);
        let table = start + Reader::new(&data).u64_le(start + 64).unwrap() as usize;
        data[table + 12..table + 16].copy_from_slice(&u32::MAX.to_le_bytes());
        let report = inspect(&data, None).unwrap().pclntab.unwrap();
        assert_eq!(report.functions.len(), 1);
        assert_eq!(report.functions[0].name, fx::GO_FUNCS[0].0);
        assert!(report.truncated);
        assert_eq!(functions(&data), report.functions);
    }

    #[test]
    fn pclntab_entries_cannot_borrow_bytes_after_the_section() {
        for data in [fx::pclntab_120(0x1000, &[("main.main", 0)]), fx::pclntab_116(0x1000, &[("main.main", 0)])] {
            let r = Reader::new(&data);
            let field = if r.u32_le(0) == Some(0xffff_fff1) { 64 } else { 56 };
            let functab = r.u64_le(field).unwrap() as usize;
            assert!(read_pclntab(&r, 0, functab + 4, "section", None, None).is_none());
            let complete = read_pclntab(&r, 0, data.len(), "section", None, None).unwrap();
            assert_eq!(complete.functions[0].name, "main.main");
        }
    }

    #[test]
    fn pclntab_names_must_terminate_inside_the_section() {
        let mut data = fx::pclntab_120(0x1000, &[("main.main", 0)]);
        let name = data.len();
        data[32..40].copy_from_slice(&(name as u64).to_le_bytes());
        data.extend_from_slice(b"main.outside\0");
        let r = Reader::new(&data);
        let partial = read_pclntab(&r, 0, name + 4, "section", None, None).unwrap();
        assert!(partial.functions.is_empty());
        assert!(!partial.warnings.is_empty());
        let complete = read_pclntab(&r, 0, data.len(), "section", None, None).unwrap();
        assert_eq!(complete.functions[0].name, "main.outside");
    }

    fn pclntab_section(data: &[u8]) -> (usize, usize, usize) {
        let r = Reader::new(data);
        let header = r.u64_le(40).unwrap() as usize + 2 * 64;
        (header, r.u64_le(header + 24).unwrap() as usize, r.u64_le(header + 32).unwrap() as usize)
    }

    #[test]
    fn pclntab_scan_does_not_widen_a_rejected_named_section() {
        let original = fx::go_elf();
        let (header, start, _) = pclntab_section(&original);
        let functab = Reader::new(&original).u64_le(start + 64).unwrap() as usize;
        for size in [functab + 4, 0] {
            let mut data = original.clone();
            data[header + 32..header + 40].copy_from_slice(&(size as u64).to_le_bytes());
            let report = inspect(&data, None).unwrap();
            assert!(report.pclntab.is_none(), "declared size={size}");
            assert!(functions(&data).is_empty(), "declared size={size}");
        }
    }

    #[test]
    fn pclntab_scan_keeps_the_named_section_end_for_interior_headers() {
        let mut data = fx::go_elf();
        let (header, start, size) = pclntab_section(&data);
        let table = data[start..start + size].to_vec();
        let functab = Reader::new(&table).u64_le(64).unwrap() as usize;
        data[start..start + 8].fill(0);
        data[start + 8..start + 8 + size].copy_from_slice(&table);
        data[header + 32..header + 40].copy_from_slice(&((size + 8) as u64).to_le_bytes());
        let complete = inspect(&data, None).unwrap().pclntab.unwrap();
        assert_eq!(complete.located_by, "scan");
        assert_eq!(complete.functions.len(), fx::GO_FUNCS.len());

        data[header + 32..header + 40].copy_from_slice(&((8 + functab + 4) as u64).to_le_bytes());
        assert!(inspect(&data, None).unwrap().pclntab.is_none());
        assert!(functions(&data).is_empty());
    }

    fn named_pclntab_with_unreadable_names(interior: bool) -> (Vec<u8>, usize, GoPclntab) {
        let mut data = fx::go_elf();
        let (header, start, size) = pclntab_section(&data);
        let recovered = start + size;
        let mut end = recovered;
        if interior {
            let table = data[start..recovered].to_vec();
            data[recovered..recovered + size].copy_from_slice(&table);
            end += size;
        }
        // The name base is inside the section, but no complete name follows it.
        data[end] = b'x';
        data[start + 32..start + 40].copy_from_slice(&((end - start) as u64).to_le_bytes());
        data[header + 32..header + 40].copy_from_slice(&((end + 1 - start) as u64).to_le_bytes());
        let partial = read_pclntab(&Reader::new(&data), start, end + 1 - start, "section", Some(".gopclntab".into()), None).unwrap();
        assert_eq!(partial.declared_functions, fx::GO_FUNCS.len() as u64);
        assert!(partial.functions.is_empty());
        assert!(!partial.warnings.is_empty());
        (data, recovered, partial)
    }

    #[test]
    fn pclntab_scan_recovers_after_an_unreadable_named_table() {
        let (data, interior, _) = named_pclntab_with_unreadable_names(true);
        let recovered = inspect(&data, None).unwrap().pclntab.unwrap();
        assert_eq!(recovered.located_by, "scan");
        assert_eq!(recovered.offset, interior as u64);
        let expected: Vec<_> = fx::GO_FUNCS.iter().map(|(name, off)| GoFunction { name: (*name).to_string(), entry: fx::GO_TEXT_VA + *off }).collect();
        assert_eq!(recovered.functions, expected);
        assert!(recovered.warnings.is_empty());
        assert_eq!(functions(&data), recovered.functions);
    }

    #[test]
    fn pclntab_scan_preserves_an_unreadable_named_table_without_replacement() {
        let (data, _, partial) = named_pclntab_with_unreadable_names(false);
        assert_eq!(inspect(&data, None).unwrap().pclntab, Some(partial));
        assert!(functions(&data).is_empty());
    }

    #[test]
    fn pclntab_scan_respects_unnamed_section_entry_bounds() {
        let mut data = fx::go_elf();
        let (header, start, _) = pclntab_section(&data);
        data[header..header + 4].fill(0);
        // A rejected header in .text must stop constraining candidates after that section.
        let prefix = data[start..start + 8].to_vec();
        data[0x1000..0x1008].copy_from_slice(&prefix);
        let complete = inspect(&data, None).unwrap().pclntab.unwrap();
        assert_eq!(complete.located_by, "scan");
        let expected: Vec<_> = fx::GO_FUNCS.iter().map(|(name, off)| GoFunction { name: (*name).to_string(), entry: fx::GO_TEXT_VA + *off }).collect();
        assert_eq!(complete.functions, expected);

        let functab = Reader::new(&data).u64_le(start + 64).unwrap() as usize;
        data[header + 32..header + 40].copy_from_slice(&((functab + 4) as u64).to_le_bytes());
        assert!(inspect(&data, None).unwrap().pclntab.is_none());
        assert!(functions(&data).is_empty());
    }

    #[test]
    fn pclntab_scan_requires_names_to_end_inside_unnamed_sections() {
        let mut data = fx::go_elf();
        let (header, start, size) = pclntab_section(&data);
        let r = Reader::new(&data);
        let names = r.u64_le(start + 32).unwrap() as usize;
        let functab = r.u64_le(start + 64).unwrap() as usize;
        let names = data[start + names..start + functab].to_vec();
        data[start + size..start + size + names.len()].copy_from_slice(&names);
        data[start + 32..start + 40].copy_from_slice(&(size as u64).to_le_bytes());
        data[header..header + 4].fill(0);
        data[header + 32..header + 40].copy_from_slice(&((size + names.len()) as u64).to_le_bytes());
        // Keep the enclosing PT_LOAD consistent with the extended section.
        let mapped_size = (start + size + names.len() - 0x1000) as u64;
        data[0x60..0x68].copy_from_slice(&mapped_size.to_le_bytes());
        data[0x68..0x70].copy_from_slice(&mapped_size.to_le_bytes());
        let complete = inspect(&data, None).unwrap().pclntab.unwrap();
        assert_eq!(complete.located_by, "scan");
        let expected: Vec<_> = fx::GO_FUNCS.iter().map(|(name, off)| GoFunction { name: (*name).to_string(), entry: fx::GO_TEXT_VA + *off }).collect();
        assert_eq!(complete.functions, expected);

        let first_name_len = fx::GO_FUNCS[0].0.len();
        assert_eq!(data[start + size + first_name_len], 0);
        data[header + 32..header + 40].copy_from_slice(&((size + first_name_len) as u64).to_le_bytes());
        assert!(inspect(&data, None).unwrap().pclntab.is_none());
        assert!(functions(&data).is_empty());
    }

    #[test]
    fn pclntab_scan_uses_the_shortest_overlapping_section_end() {
        let mut original = fx::go_elf();
        let (header, start, size) = pclntab_section(&original);
        let text_header = header - 64;
        let text_start = Reader::new(&original).u64_le(text_header + 24).unwrap() as usize;
        original[header..header + 4].fill(0);
        original[text_header + 32..text_header + 40].copy_from_slice(&((start + size - text_start) as u64).to_le_bytes());
        let complete = inspect(&original, None).unwrap().pclntab.unwrap();
        assert_eq!(complete.located_by, "scan");
        assert_eq!(complete.functions.len(), fx::GO_FUNCS.len());

        let functab = Reader::new(&original).u64_le(start + 64).unwrap() as usize;
        for (section, section_start) in [(header, start), (text_header, text_start)] {
            let mut data = original.clone();
            let size = start + functab + 4 - section_start;
            data[section + 32..section + 40].copy_from_slice(&(size as u64).to_le_bytes());
            assert!(inspect(&data, None).unwrap().pclntab.is_none());
            assert!(functions(&data).is_empty());
        }
    }

    #[test]
    fn pclntab_scan_keeps_whole_file_fallback_outside_known_sections() {
        for stripped in [false, true] {
            let mut data = fx::go_elf();
            let (header, _, _) = pclntab_section(&data);
            data[header..header + 4].fill(0);
            data[header + 32..header + 40].fill(0);
            if stripped {
                data[40..48].fill(0); // e_shoff
                data[60..64].fill(0); // e_shnum and e_shstrndx
            }
            let complete = inspect(&data, None).unwrap().pclntab.unwrap();
            assert_eq!(complete.located_by, "scan");
            let expected: Vec<_> = fx::GO_FUNCS.iter().map(|(name, off)| GoFunction { name: (*name).to_string(), entry: fx::GO_TEXT_VA + *off }).collect();
            assert_eq!(complete.functions, expected);
        }
    }

    #[test]
    fn elf_inline_buildinfo_and_pclntab_120() {
        let r = inspect(&fx::go_elf(), None).unwrap();
        assert_eq!(r.container, "elf");
        let b = r.buildinfo.as_ref().unwrap();
        assert_eq!(b.encoding, "inline");
        assert_eq!(b.go_version.as_deref(), Some(fx::GO_VERSION));
        assert_eq!(b.path.as_deref(), Some("example.com/hello"));
        assert_eq!(b.main.as_ref().unwrap().path, "example.com/hello");
        assert_eq!(b.deps.len(), 2);
        assert_eq!(b.deps[1].replace.as_ref().unwrap().path, "../local/text");
        assert!(b.settings.iter().any(|s| s.key == "-ldflags" && s.value == "-s -w \"x\""));
        assert!(b.complete, "{:?}", b);
        let p = r.pclntab.as_ref().unwrap();
        assert_eq!(p.layout, "1.20+");
        assert_eq!(p.located_by, "section");
        let names: Vec<_> = p.functions.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, fx::GO_FUNCS.iter().map(|(n, _)| *n).collect::<Vec<_>>());
        assert_eq!(p.functions[0].entry, fx::GO_TEXT_VA + fx::GO_FUNCS[0].1);
        assert!(r.packages.iter().any(|p| p.package == "main" && p.functions == 2));
        assert!(r.packages.iter().any(|p| p.package == "github.com/acme/lib"));
        assert_eq!(functions(&fx::go_elf()), p.functions);
        assert!(functions(b"ELF not a Go program").is_empty());
    }

    #[test]
    fn macho_pointer_buildinfo_and_scanned_pclntab_116() {
        let r = inspect(&fx::go_macho(), None).unwrap();
        assert_eq!(r.container, "macho");
        let b = r.buildinfo.as_ref().unwrap();
        assert_eq!(b.encoding, "pointer");
        assert_eq!(b.go_version.as_deref(), Some("go1.17.13"));
        assert_eq!(b.main.as_ref().unwrap().version, "(devel)");
        let p = r.pclntab.as_ref().unwrap();
        assert_eq!(p.layout, "1.16");
        assert_eq!(p.located_by, "scan");
        assert_eq!(p.functions.len(), fx::GO_FUNCS.len());
        assert_eq!(p.functions[1].entry, 0x1_0000_1000 + fx::GO_FUNCS[1].1);
    }

    #[test]
    fn non_go_and_quoting() {
        let r = inspect(&craft_anything_testkit::elf64(craft_anything_testkit::ElfArch::X86_64, true).0, None).unwrap();
        assert!(!r.is_go);
        assert_eq!(unquote_prefix("\"a\\tb\\x41\\u00e9\\101\" rest"), Some(("a\tbA\u{e9}A".to_string(), 20)));
        assert_eq!(unquote_prefix("\"open"), None);
        assert_eq!(unquote_prefix("\"\\x+a\""), None);
        assert_eq!(unquote_prefix("\"\\u+041\""), None);
        assert_eq!(unquote_prefix("\"\\U+0000041\""), None);
        assert_eq!(unquote_prefix("\"\\x0a\""), Some(("\n".to_string(), 6)));
        assert_eq!(package_of("github.com/a/b.(*T).M"), "github.com/a/b");
        assert_eq!(package_of("main.main"), "main");
        assert_eq!(base64(b"hi!?"), "aGkhPw==");
    }

    #[test]
    fn hostile_inputs_do_not_panic() {
        for sample in [fx::go_elf(), fx::go_macho()] {
            for m in craft_anything_testkit::mutations(&sample) {
                let _ = inspect(&m, None);
            }
            for n in 0..sample.len().min(600) {
                let _ = inspect(&sample[..n], None);
            }
        }
        for seed in 0..64 {
            let mut n = craft_anything_testkit::noise(seed, 2048);
            n[..4].copy_from_slice(b"\x7fELF");
            let _ = inspect(&n, None);
            let _ = inspect(&craft_anything_testkit::noise(seed, 512), None);
        }
    }
}
