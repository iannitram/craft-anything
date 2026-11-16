//! Native inventories: functions, segments, strings, names, documents, bookmarks; searches;
//! raw bytes; address ↔ file offset; cross-references; reading names and comments.

use super::native_common::{DEFAULT_LIMIT, address_or_cursor, ev, hex, image, name_of, native, page, params_json, select_document, spec};
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_disasm::{Xref, slot_label};
use craft_anything_evidence::{Confidence, Coverage, ErrorCode, Limitation, Location, ToolError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub fn specs() -> Vec<ToolSpec> {
    use Toolset::Native;
    let e = Effects::EVIDENCE;
    let t = Requires::Target;
    vec![
        spec(
            "native_functions_list",
            "List functions",
            "Every function the built-in engine found: entry, name, size, blocks, how it was found. Paged (offset/limit).",
            "See every function (piece of code) in the program.",
            Native,
            e,
            t,
            schema_of::<PageParams>,
            native_functions_list,
        ),
        spec(
            "native_sections_list",
            "List segments",
            "Segments and sections with address ranges, file offsets, permissions and entropy.",
            "See how the program is laid out in memory.",
            Native,
            e,
            t,
            schema_of::<DocParams>,
            native_sections_list,
        ),
        spec(
            "native_strings_browse",
            "List strings",
            "Printable strings (ASCII, UTF-8, UTF-16) in data sections with their addresses and how many places use them. Optional filters: one address, one section (`.rdata`); native_strings_find finds text. Paged.",
            "See the text inside the program.",
            Native,
            e,
            t,
            schema_of::<StringsParams>,
            native_strings_browse,
        ),
        spec(
            "native_named_addresses_list",
            "List names",
            "Named addresses: your names, functions, data symbols and import slots. Optional single-address filter. Paged.",
            "See every named place in the program.",
            Native,
            e,
            t,
            schema_of::<AddrPageParams>,
            native_named_addresses_list,
        ),
        spec(
            "native_open_documents_list",
            "List documents",
            "Open targets (documents) with ids, kinds and which one is active.",
            "See what's open.",
            Native,
            Effects::READ_ONLY,
            Requires::Nothing,
            schema_of::<NoParams>,
            native_open_documents_list,
        ),
        spec(
            "native_bookmarks_show",
            "List bookmarks",
            "Bookmarks you or an agent set on the active target.",
            "See your bookmarks.",
            Native,
            Effects::READ_ONLY,
            t,
            schema_of::<DocParams>,
            native_bookmarks_show,
        ),
        spec(
            "native_functions_search",
            "Search functions",
            "Complete, deterministic search of function names (yours and the engine's) and imports (kind \"import\", e.g. CreateWindowExW). mode literal or regex; case-insensitive unless case_sensitive. Paged.",
            "Find functions by name.",
            Native,
            e,
            t,
            schema_of::<SearchParams>,
            native_functions_search,
        ),
        spec(
            "native_strings_find",
            "Search strings",
            "Complete, deterministic search of the program's strings. mode literal or regex; case-insensitive unless case_sensitive. Paged.",
            "Find text inside the program.",
            Native,
            e,
            t,
            schema_of::<SearchParams>,
            native_strings_find,
        ),
        spec(
            "native_bytes_show",
            "Read bytes",
            "Bytes at a virtual address as hex (default 256, at most 65536), never past the end of the section.",
            "Look at the raw bytes somewhere in the program.",
            Native,
            e | Effects::CARRIES_TARGET_CONTENT,
            t,
            schema_of::<ReadBytesParams>,
            native_bytes_show,
        ),
        spec(
            "native_address_offset_show",
            "Address to file offset",
            "Map a virtual address to its offset in the original file (and the section holding it).",
            "Find where an address lives in the file on disk.",
            Native,
            e,
            t,
            schema_of::<AddrParams>,
            native_address_offset_show,
        ),
        spec(
            "native_xrefs_list",
            "Cross-references",
            "References to an address (calls, jumps, data, strings, pointers, imports) and the references made from it. Defaults to the cursor. Paged.",
            "See what uses this address.",
            Native,
            e,
            t,
            schema_of::<AddrPageParams>,
            native_xrefs_list,
        ),
        spec(
            "native_name_show",
            "Name at address",
            "The primary name at an address (your name first), or null.",
            "What is this address called?",
            Native,
            e,
            t,
            schema_of::<OptAddrParams>,
            native_name_show,
        ),
        spec(
            "native_comment_read",
            "Read comment",
            "The comment at an address, or null.",
            "Read the note at this address.",
            Native,
            Effects::READ_ONLY,
            t,
            schema_of::<OptAddrParams>,
            native_comment_read,
        ),
        spec(
            "native_inline_comment_read",
            "Read inline comment",
            "The inline comment at an address, or null.",
            "Read the short note on this line.",
            Native,
            Effects::READ_ONLY,
            t,
            schema_of::<OptAddrParams>,
            native_inline_comment_read,
        ),
    ]
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoParams {}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DocParams {
    /// Open target (document) to use; the active one when omitted.
    #[serde(default)]
    pub document: Option<String>,
}

fn default_limit() -> usize {
    DEFAULT_LIMIT
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PageParams {
    /// Open target (document) to use; the active one when omitted.
    #[serde(default)]
    pub document: Option<String>,
    /// First item to return.
    #[serde(default)]
    pub offset: usize,
    /// Items per page (default 200, at most 5000).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StringsParams {
    /// Open target (document) to use; the active one when omitted.
    #[serde(default)]
    pub document: Option<String>,
    /// Only the string at this address (0x-hex).
    #[serde(default)]
    pub address: Option<String>,
    /// Only strings in this section (`.rdata`, `__cstring`; case doesn't matter).
    #[serde(default)]
    pub section: Option<String>,
    /// First item to return.
    #[serde(default)]
    pub offset: usize,
    /// Items per page (default 200, at most 5000).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddrPageParams {
    /// Open target (document) to use; the active one when omitted.
    #[serde(default)]
    pub document: Option<String>,
    /// Only this address (0x-hex).
    #[serde(default)]
    pub address: Option<String>,
    /// First item to return.
    #[serde(default)]
    pub offset: usize,
    /// Items per page (default 200, at most 5000).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddrParams {
    /// Open target (document) to use; the active one when omitted.
    #[serde(default)]
    pub document: Option<String>,
    /// Virtual address (0x-hex).
    pub address: String,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OptAddrParams {
    /// Open target (document) to use; the active one when omitted.
    #[serde(default)]
    pub document: Option<String>,
    /// Virtual address (0x-hex); the cursor when omitted.
    #[serde(default)]
    pub address: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    #[default]
    Literal,
    Regex,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchParams {
    /// Open target (document) to use; the active one when omitted.
    #[serde(default)]
    pub document: Option<String>,
    /// What to look for.
    pub pattern: String,
    /// literal (default) or regex.
    #[serde(default)]
    pub mode: SearchMode,
    /// Match case exactly (default false).
    #[serde(default)]
    pub case_sensitive: bool,
    /// First match to return.
    #[serde(default)]
    pub offset: usize,
    /// Matches per page (default 200, at most 5000).
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_len() -> usize {
    256
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadBytesParams {
    /// Open target (document) to use; the active one when omitted.
    #[serde(default)]
    pub document: Option<String>,
    /// Virtual address (0x-hex).
    pub address: String,
    /// How many bytes (1..=65536, default 256).
    #[serde(default = "default_len")]
    pub length: usize,
}

/// A compiled search matcher.
pub enum Matcher {
    Literal { needle: String, case_sensitive: bool },
    Regex(regex::Regex),
}

impl Matcher {
    pub fn new(pattern: &str, mode: SearchMode, case_sensitive: bool) -> Result<Matcher, ToolError> {
        if pattern.is_empty() {
            return Err(ToolError::invalid("pattern is empty"));
        }
        match mode {
            SearchMode::Literal => Ok(Matcher::Literal { needle: if case_sensitive { pattern.to_string() } else { pattern.to_lowercase() }, case_sensitive }),
            SearchMode::Regex => regex::RegexBuilder::new(pattern)
                .case_insensitive(!case_sensitive)
                .size_limit(1 << 20)
                .dfa_size_limit(4 << 20)
                .build()
                .map(Matcher::Regex)
                .map_err(|e| ToolError::invalid(format!("bad regex: {e}"))),
        }
    }

    pub fn is_match(&self, s: &str) -> bool {
        match self {
            Matcher::Literal { needle, case_sensitive: true } => s.contains(needle.as_str()),
            Matcher::Literal { needle, case_sensitive: false } => s.to_lowercase().contains(needle.as_str()),
            Matcher::Regex(r) => r.is_match(s),
        }
    }
}

fn native_functions_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: PageParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let st = native(ctx)?;
    let t = ctx.session.active()?;
    let rows: Vec<Value> = st
        .prog
        .functions
        .values()
        .map(|f| {
            json!({
                "address": hex(f.entry),
                "name": name_of(t, &st.prog, f.entry).unwrap_or_else(|| f.name.clone()),
                "size": f.size(),
                "blocks": f.blocks.len(),
                "instructions": f.insn_count,
                "found_by": f.discovered_by,
                "name_source": if t.annotations.names.contains_key(&f.entry) { "user" } else { f.name_source },
                "thunk_of": f.thunk_of,
            })
        })
        .collect();
    let (items, cov) = page(&rows, p.offset, p.limit);
    let cov = super::native_common::program_coverage(&st.prog, cov);
    let e = ev(ctx, "native_functions_list", params_json(&p))?
        .result(json!({"functions": items, "total": rows.len()}))
        .summary(format!("{} functions (showing {}).", rows.len(), items.len()))
        .confidence(Confidence::Derived)
        .coverage(cov)
        .limitations(st.prog.limitations.iter().map(|(c, m)| Limitation::new(c.clone(), m.clone())))
        .build()?;
    Ok(ToolOutput::evidence(e))
}

fn native_sections_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: DocParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let img = image(ctx)?;
    let b = &img.0.bin;
    let segs: Vec<Value> = b
        .segments
        .iter()
        .map(|s| json!({"name": s.name, "start": hex(s.addr), "end": hex(s.addr.saturating_add(s.size)), "size": s.size, "file_offset": s.file_offset, "file_size": s.file_size, "read": s.read, "write": s.write, "exec": s.exec}))
        .collect();
    let secs: Vec<Value> = b
        .sections
        .iter()
        .map(|s| {
            json!({"name": s.name, "segment": s.segment, "start": hex(s.addr), "end": hex(s.addr.saturating_add(s.size)), "size": s.size, "file_offset": s.file_offset, "file_size": s.file_size, "read": s.read, "write": s.write, "exec": s.exec, "kind": s.kind, "entropy": s.entropy})
        })
        .collect();
    let n = segs.len() + secs.len();
    let e = ev(ctx, "native_sections_list", params_json(&p))?
        .result(json!({"segments": segs, "sections": secs}))
        .summary(format!("{} segments and {} sections.", b.segments.len(), b.sections.len()))
        .coverage(Coverage::complete(n as u64))
        .build()?;
    Ok(ToolOutput::evidence(e))
}

fn native_strings_browse(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: StringsParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let st = native(ctx)?;
    let filter = p.address.as_deref().map(super::parse_address).transpose()?;
    let section = p.section.as_deref().map(str::trim).filter(|s| !s.is_empty());
    if let Some(want) = section
        && !st.prog.strings.iter().any(|s| s.section.eq_ignore_ascii_case(want))
    {
        let mut have: Vec<&str> = st.prog.strings.iter().map(|s| s.section.as_str()).collect();
        have.sort_unstable();
        have.dedup();
        return Err(ToolError::new(ErrorCode::NotFound, format!("No strings in a section named {want:?}.")).with_remediation(if have.is_empty() {
            "This program has no strings listed.".to_string()
        } else {
            format!("Sections with strings: {}.", have.join(", "))
        }));
    }
    let rows: Vec<Value> = st
        .prog
        .strings
        .iter()
        .filter(|s| filter.is_none_or(|a| a >= s.addr && a < s.addr.saturating_add(s.byte_len.max(1))))
        .filter(|s| section.is_none_or(|want| s.section.eq_ignore_ascii_case(want)))
        .map(|s| json!({"address": hex(s.addr), "text": s.text, "encoding": s.encoding, "section": s.section, "file_offset": s.file_offset, "references": st.prog.xrefs_to.get(&s.addr).map(Vec::len).unwrap_or(0)}))
        .collect();
    let (items, cov) = page(&rows, p.offset, p.limit);
    let mut b = ev(ctx, "native_strings_browse", params_json(&p))?
        .result(json!({"strings": items, "total": rows.len()}))
        .summary(format!("{} strings (showing {}).", rows.len(), items.len()))
        .coverage(cov);
    if let Some(a) = filter {
        b = b.location(Location::address(a));
    }
    b = b.limitation(Limitation::new("data_sections_only", "strings are read from data sections; text built at run time or stored compressed isn't listed"));
    Ok(ToolOutput::evidence(b.build()?))
}

fn native_named_addresses_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: AddrPageParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let st = native(ctx)?;
    let t = ctx.session.active()?;
    let filter = p.address.as_deref().map(super::parse_address).transpose()?;
    let mut names: std::collections::BTreeMap<u64, (String, &str)> = std::collections::BTreeMap::new();
    for f in st.prog.functions.values() {
        names.insert(f.entry, (f.name.clone(), "function"));
    }
    for (a, n) in &st.prog.data_names {
        names.entry(*a).or_insert((n.clone(), "data"));
    }
    for (a, s) in &st.prog.import_slots {
        names.entry(*a).or_insert((slot_label(s), "import"));
    }
    for (a, n) in &t.annotations.names {
        names.insert(*a, (n.clone(), "user"));
    }
    let rows: Vec<Value> =
        names.iter().filter(|(a, _)| filter.is_none_or(|f| f == **a)).map(|(a, (n, k))| json!({"address": hex(*a), "name": n, "kind": k})).collect();
    let (items, cov) = page(&rows, p.offset, p.limit);
    let e = ev(ctx, "native_named_addresses_list", params_json(&p))?
        .result(json!({"names": items, "total": rows.len()}))
        .summary(format!("{} named addresses (showing {}).", rows.len(), items.len()))
        .coverage(cov)
        .build()?;
    Ok(ToolOutput::evidence(e))
}

fn native_open_documents_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let _: NoParams = parse_params(params)?;
    let active = ctx.session.active_id().cloned();
    let docs: Vec<Value> = ctx
        .session
        .targets()
        .map(|t| json!({"document": t.target.id, "name": t.target.name, "kind": t.target.kind, "description": t.target.kind.plain(), "active": Some(&t.target.id) == active.as_ref(), "sha256": t.target.sha256}))
        .collect();
    Ok(ToolOutput::Value(json!({"documents": docs, "active": active})))
}

fn native_bookmarks_show(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: DocParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let t = ctx.session.active()?;
    let marks: Vec<Value> = t.annotations.bookmarks.iter().map(|(a, n)| json!({"address": hex(*a), "name": n})).collect();
    Ok(ToolOutput::Value(json!({"bookmarks": marks})))
}

fn native_functions_search(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: SearchParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let m = Matcher::new(&p.pattern, p.mode, p.case_sensitive)?;
    let st = native(ctx)?;
    let t = ctx.session.active()?;
    let mut rows = Vec::new();
    for (i, f) in st.prog.functions.values().enumerate() {
        if i % 4096 == 0 {
            ctx.cancel.check()?;
        }
        let n = name_of(t, &st.prog, f.entry).unwrap_or_else(|| f.name.clone());
        let demangled = st.img.bin.symbols.iter().find(|s| s.addr == f.entry && s.demangled.is_some()).and_then(|s| s.demangled.clone());
        if m.is_match(&n) || demangled.as_deref().is_some_and(|d| m.is_match(d)) {
            rows.push(json!({"kind": "function", "address": hex(f.entry), "name": n, "demangled": demangled, "size": f.size()}));
        }
    }
    let functions = rows.len();
    // Imports aren't functions here, but people search for them by name (CreateWindowExW).
    rows.extend(
        st.img
            .bin
            .imports
            .iter()
            .filter(|i| m.is_match(&i.name))
            .map(|i| json!({"kind": "import", "address": i.slot.map(hex), "name": i.name, "library": i.library})),
    );
    let (items, cov) = page(&rows, p.offset, p.limit);
    let cov = super::native_common::program_coverage(&st.prog, cov);
    let e = ev(ctx, "native_functions_search", params_json(&p))?
        .result(json!({"matches": items, "total": rows.len(), "functions": functions, "imports": rows.len() - functions}))
        .summary(format!("{functions} functions and {} imports match \"{}\".", rows.len() - functions, p.pattern))
        .confidence(Confidence::Derived)
        .coverage(cov)
        .limitations(
            st.prog.limitations.iter().filter(|(c, _)| c == "function_limit" || c == "instruction_limit").map(|(c, m)| Limitation::new(c.clone(), m.clone())),
        )
        .build()?;
    Ok(ToolOutput::evidence(e))
}

/// Most references listed for one matching string; `reference_count` stays complete within the analysis.
const MAX_STRING_REFERENCES: usize = 32;

/// The first references, the complete count, and whether the list was cut.
fn string_references(refs: &[Xref], function_of: impl Fn(u64) -> Option<String>) -> (Vec<Value>, usize, bool) {
    let users = refs.iter().take(MAX_STRING_REFERENCES).map(|x| json!({"from": hex(x.from), "function": function_of(x.from)})).collect();
    (users, refs.len(), refs.len() > MAX_STRING_REFERENCES)
}

/// The display cap's limitation, which also says how far `reference_count` can be trusted.
fn references_capped_limitation(capped: bool, analysis_capped: bool) -> Option<Limitation> {
    capped.then(|| {
        let completeness = if analysis_capped {
            "the analysis xref limit was reached, so reference_count may be incomplete too."
        } else {
            "reference_count is complete within this analysis, which can miss references made at run time."
        };
        Limitation::new("references_capped", format!("Only the first {MAX_STRING_REFERENCES} references are listed per string; {completeness}"))
    })
}

/// The analysis's own cross-reference limit, when it kept only the first cross-references.
fn xref_limit_limitation(limitations: &[(String, String)]) -> Option<Limitation> {
    limitations.iter().find(|(code, _)| code == "xref_limit").map(|(code, message)| Limitation::new(code.clone(), message.clone()))
}

fn native_strings_find(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: SearchParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let m = Matcher::new(&p.pattern, p.mode, p.case_sensitive)?;
    let st = native(ctx)?;
    let mut rows = Vec::new();
    let mut references_capped = false;
    for (i, s) in st.prog.strings.iter().enumerate() {
        if i % 4096 == 0 {
            ctx.cancel.check()?;
        }
        if m.is_match(&s.text) {
            let refs = st.prog.xrefs_to.get(&s.addr).map(Vec::as_slice).unwrap_or(&[]);
            let (users, total, capped) = string_references(refs, |from| st.prog.function_containing(from).map(|f| f.name.clone()));
            references_capped |= capped;
            rows.push(json!({
                "address": hex(s.addr),
                "text": s.text,
                "section": s.section,
                "references": users,
                "reference_count": total,
                "references_capped": capped,
            }));
        }
    }
    let (items, cov) = page(&rows, p.offset, p.limit);
    let mut b = ev(ctx, "native_strings_find", params_json(&p))?
        .result(json!({"matches": items, "total": rows.len()}))
        .summary(format!("{} strings match \"{}\".", rows.len(), p.pattern))
        .coverage(cov)
        .limitation(Limitation::new("data_sections_only", "only strings stored in the file are searched (absence is not proof)"));
    let xref_limit = xref_limit_limitation(&st.prog.limitations);
    let analysis_capped = xref_limit.is_some();
    if let Some(l) = xref_limit {
        b = b.limitation(l);
    }
    if let Some(l) = references_capped_limitation(references_capped, analysis_capped) {
        b = b.limitation(l);
    }
    let e = b.build()?;
    Ok(ToolOutput::evidence(e))
}

fn native_bytes_show(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ReadBytesParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    if p.length == 0 || p.length > 65_536 {
        return Err(ToolError::invalid("length must be 1..=65536"));
    }
    let a = super::parse_address(&p.address)?;
    let img = image(ctx)?;
    let bytes = img.0.read(a, p.length).ok_or_else(|| {
        ToolError::invalid(format!("{a:#x} isn't backed by bytes in the file (unmapped or zero-filled)"))
            .with_remediation("Pick an address inside a section from native_sections_list.")
    })?;
    let ascii: String = bytes.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '.' }).collect();
    let truncated = bytes.len() < p.length;
    let mut b = ev(ctx, "native_bytes_show", params_json(&p))?
        .result(json!({"address": hex(a), "length": bytes.len(), "hex": hex::encode(bytes), "ascii": ascii, "file_offset": img.0.bin.va_to_offset(a)}))
        .summary(format!("{} bytes at {a:#x}.", bytes.len()))
        .location(Location::address(a))
        .coverage(Coverage::complete(bytes.len() as u64));
    if truncated {
        b = b.limitation(Limitation::new("truncated", format!("only {} bytes are file-backed before the section ends", bytes.len())));
    }
    Ok(ToolOutput::evidence(b.build()?))
}

fn native_address_offset_show(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: AddrParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let a = super::parse_address(&p.address)?;
    let img = image(ctx)?;
    let off = img.0.bin.va_to_offset(a).ok_or_else(|| ToolError::invalid(format!("{a:#x} has no file offset (unmapped, or zero-filled at load time)")))?;
    let sec = img.0.bin.section_at(a).map(|s| s.name.clone());
    let sha = ctx.session.active()?.target.sha256.clone();
    let e = ev(ctx, "native_address_offset_show", params_json(&p))?
        .result(json!({"address": hex(a), "file_offset": off, "file_offset_hex": hex(off), "section": sec, "file_sha256": sha}))
        .summary(format!("{a:#x} is at file offset {off:#x}{}.", sec.as_deref().map(|s| format!(" in {s}")).unwrap_or_default()))
        .location(Location::address(a))
        .location(Location::new("file_offset", hex(off)))
        .build()?;
    Ok(ToolOutput::evidence(e))
}

fn native_xrefs_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: AddrPageParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let a = address_or_cursor(ctx.session, &p.address)?;
    let st = native(ctx)?;
    let t = ctx.session.active()?;
    let to: Vec<Value> = st
        .prog
        .xrefs_to
        .get(&a)
        .map(|v| v.iter().map(|x| json!({"from": hex(x.from), "kind": x.kind, "function": st.prog.function_containing(x.from).map(|f| name_of(t, &st.prog, f.entry).unwrap_or_else(|| f.name.clone()))})).collect())
        .unwrap_or_default();
    let from: Vec<Value> = st
        .prog
        .xrefs_from
        .get(&a)
        .map(|v| v.iter().map(|x| json!({"to": hex(x.to), "kind": x.kind, "name": name_of(t, &st.prog, x.to)})).collect())
        .unwrap_or_default();
    let (items, cov) = page(&to, p.offset, p.limit);
    let e = ev(ctx, "native_xrefs_list", params_json(&p))?
        .result(json!({"address": hex(a), "name": name_of(t, &st.prog, a), "references_to": items, "total_to": to.len(), "references_from": from}))
        .summary(format!("{a:#x} is referenced from {} places and refers to {}.", to.len(), from.len()))
        .confidence(Confidence::Derived)
        .coverage(cov)
        .location(Location::address(a))
        .limitation(Limitation::new("static_references", "references made through computed pointers at run time are not seen"))
        .build()?;
    Ok(ToolOutput::evidence(e))
}

fn native_name_show(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: OptAddrParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let a = address_or_cursor(ctx.session, &p.address)?;
    let st = native(ctx)?;
    let t = ctx.session.active()?;
    let name = name_of(t, &st.prog, a);
    let source = if t.annotations.names.contains_key(&a) { "user" } else { "engine" };
    let e = ev(ctx, "native_name_show", params_json(&p))?
        .result(json!({"address": hex(a), "name": name, "source": name.as_ref().map(|_| source)}))
        .summary(match &name {
            Some(n) => format!("{a:#x} is {n}."),
            None => format!("{a:#x} has no name."),
        })
        .location(Location::address(a))
        .build()?;
    Ok(ToolOutput::evidence(e))
}

fn read_comment(ctx: &mut Ctx<'_>, params: Value, inline: bool) -> Result<ToolOutput, ToolError> {
    let p: OptAddrParams = parse_params(params)?;
    select_document(ctx, &p.document)?;
    let a = address_or_cursor(ctx.session, &p.address)?;
    let t = ctx.session.active()?;
    let map = if inline { &t.annotations.inline_comments } else { &t.annotations.comments };
    Ok(ToolOutput::Value(json!({"address": hex(a), "comment": map.get(&a)})))
}

fn native_comment_read(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    read_comment(ctx, params, false)
}

fn native_inline_comment_read(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    read_comment(ctx, params, true)
}

#[cfg(test)]
mod tests {
    use super::super::native_common::{NativeState, test_support::*};
    use super::*;
    use craft_anything_disasm::{Image, Limits, NoControl, Program, Xref, XrefKind};
    use craft_anything_testkit as tk;
    use serde_json::json;

    #[test]
    fn listings_and_searches() {
        let (elf, l) = tk::elf64(tk::ElfArch::X86_64, true);
        let e = engine_with("prog", &elf);
        let r = call(&e, "native_functions_list", json!({"limit": 2}));
        assert_eq!(r["result"]["total"], 4);
        assert_eq!(r["coverage"]["next_offset"], 2);
        let r = call(&e, "native_sections_list", json!({}));
        assert!(r["result"]["sections"].as_array().unwrap().iter().any(|s| s["name"] == ".text"));
        let r = call(&e, "native_strings_browse", json!({}));
        assert!(r["result"]["strings"].as_array().unwrap().iter().any(|s| s["text"] == tk::HELLO));
        let r = call(&e, "native_strings_browse", json!({"address": format!("{:#x}", l.rodata_va + 3)}));
        assert_eq!(r["result"]["total"], 1);
        let all = call(&e, "native_strings_browse", json!({}))["result"]["total"].clone();
        let section = r["result"]["strings"][0]["section"].as_str().unwrap().to_uppercase();
        let one = call(&e, "native_strings_browse", json!({"section": section}));
        let strings = one["result"]["strings"].as_array().unwrap();
        assert!(!strings.is_empty() && one["result"]["total"].as_u64() < all.as_u64(), "only that section's strings");
        assert!(strings.iter().all(|s| s["section"].as_str().unwrap().eq_ignore_ascii_case(&section)), "any case");
        let err = call_err(&e, "native_strings_browse", json!({"section": ".nope"}));
        assert!(err.remediation.unwrap_or_default().contains("Sections with strings"), "names the sections that have strings");
        let r = call(&e, "native_named_addresses_list", json!({}));
        assert!(r["result"]["names"].as_array().unwrap().iter().any(|n| n["name"] == "puts@got"));
        let r = call(&e, "native_functions_search", json!({"pattern": "^h.*r$", "mode": "regex"}));
        assert_eq!(r["result"]["matches"][0]["name"], "helper");
        assert_eq!(r["result"]["matches"][0]["kind"], "function");
        // Imports are hits too, marked as such.
        let r = call(&e, "native_functions_search", json!({"pattern": "puts"}));
        let hit = r["result"]["matches"].as_array().unwrap().iter().find(|m| m["kind"] == "import").cloned().unwrap();
        assert_eq!(hit["name"], "puts");
        let r = call(&e, "native_strings_find", json!({"pattern": "CRAFT"}));
        assert_eq!(r["result"]["total"], 1);
        let hit = &r["result"]["matches"][0];
        assert_eq!(hit["reference_count"].as_u64().unwrap() as usize, hit["references"].as_array().unwrap().len());
        assert_eq!(hit["references_capped"], false);
        let r = call(&e, "native_strings_find", json!({"pattern": "CRAFT", "case_sensitive": true}));
        assert_eq!(r["result"]["total"], 0);
        assert_eq!(call_err(&e, "native_strings_find", json!({"pattern": "(", "mode": "regex"})).code, craft_anything_evidence::ErrorCode::BadInput);
        let r = call(&e, "native_bytes_show", json!({"address": format!("{:#x}", l.code_va), "length": 4}));
        assert_eq!(r["result"]["hex"], "554889e5");
        assert!(call_err(&e, "native_bytes_show", json!({"address": "0x10"})).message.contains("isn't backed"));
        let r = call(&e, "native_address_offset_show", json!({"address": format!("{:#x}", l.code_va)}));
        assert_eq!(r["result"]["file_offset"], tk::ELF_TEXT_OFF);
        let helper = format!("{:#x}", l.code_va + tk::x64::HELPER);
        let r = call(&e, "native_xrefs_list", json!({"address": helper}));
        assert_eq!(r["result"]["references_to"][0]["function"], "main");
        let r = call(&e, "native_name_show", json!({"address": helper}));
        assert_eq!(r["result"]["name"], "helper");
        let r = call(&e, "native_comment_read", json!({"address": helper}));
        assert!(r["result"]["comment"].is_null());
        let r = call(&e, "native_open_documents_list", json!({}));
        assert_eq!(r["result"]["documents"].as_array().unwrap().len(), 1);
        assert!(call_err(&e, "native_functions_list", json!({"bogus": 1})).message.contains("unknown field"));
        assert!(call_err(&e, "native_functions_list", json!({"document": "nope"})).remediation.is_some());
    }

    /// A string with more than 32 references keeps its full count and says the list was cut.
    #[test]
    fn string_references_report_the_full_count_when_capped() {
        let refs: Vec<Xref> = (0..33).map(|i| Xref { from: 0x1000 + i, to: 0x400240, kind: XrefKind::Data }).collect();
        let (users, total, capped) = string_references(&refs, |from| (from % 2 == 0).then(|| "main".to_string()));
        assert_eq!(total, 33);
        assert_eq!(users.len(), 32);
        assert!(capped);
        assert_eq!(users[0]["function"], "main");
        assert!(users[1]["function"].is_null());
        let (_, total, capped) = string_references(&refs[..32], |_| None);
        assert_eq!(total, 32);
        assert!(!capped);
        let plain = references_capped_limitation(true, false).unwrap();
        assert!(plain.message.contains("reference_count is complete within this analysis"), "{}", plain.message);
        let limited = references_capped_limitation(true, true).unwrap();
        assert!(limited.message.contains("analysis xref limit") && !limited.message.contains("complete within this analysis"), "{}", limited.message);
        assert!(references_capped_limitation(false, false).is_none());
        assert!(references_capped_limitation(false, true).is_none());
        let lims = vec![("xref_limit".to_string(), "kept the first 10 cross-references".to_string())];
        assert_eq!(xref_limit_limitation(&lims).unwrap().code, "xref_limit");
        assert!(xref_limit_limitation(&[]).is_none());
    }

    /// An image whose entry function references the first string `refs` times and the second
    /// once. Both strings are found by the same search, so its pages can be checked.
    fn image_with_many_string_references(refs: usize) -> (Image, String, String) {
        const BASE: u64 = 0x1000;
        const FIRST: u64 = 0x2000;
        const SECOND: u64 = 0x2040;
        let first = "STATIC_REFERENCE_TARGET";
        let second = "STATIC_REFERENCE_OTHER";
        let mut bytes = vec![0u8; 0x1080];
        let lea = |bytes: &mut [u8], at: u64, to: u64| {
            let off = (at - BASE) as usize;
            bytes[off..off + 3].copy_from_slice(&[0x48, 0x8d, 0x3d]);
            bytes[off + 3..off + 7].copy_from_slice(&((to as i64 - (at + 7) as i64) as i32).to_le_bytes());
        };
        // The second string's only reference comes first, so it survives the analysis limit.
        lea(&mut bytes, BASE, SECOND);
        for i in 0..refs {
            lea(&mut bytes, BASE + 7 * (i as u64 + 1), FIRST);
        }
        bytes[7 * (refs + 1)] = 0xc3;
        bytes[FIRST as usize - BASE as usize..][..first.len()].copy_from_slice(first.as_bytes());
        bytes[SECOND as usize - BASE as usize..][..second.len()].copy_from_slice(second.as_bytes());
        (Image::raw(std::sync::Arc::from(bytes), craft_anything_formats::Arch::X86_64, BASE), first.to_string(), second.to_string())
    }

    /// The analysis xref limit and the 32-reference display cap both reach the tool's evidence on
    /// both pages, and a reference outside every function keeps a null attribution.
    #[test]
    fn string_reference_limits_reach_the_evidence_across_pages() {
        let (img, first, second) = image_with_many_string_references(40);
        // 41 references, a 40-xref analysis limit: the first string loses its last reference to
        // the limit, stays above the display cap, and the second string's one reference is kept.
        let mut prog = Program::analyze(&img, &Limits { max_xrefs: 40, ..Limits::default() }, &NoControl).unwrap();
        assert!(prog.limitations.iter().any(|(code, _)| code == "xref_limit"), "the fixture hits the analysis limit");
        let second_addr = prog.strings.iter().find(|s| s.text == second).unwrap().addr;
        prog.xrefs_to.get_mut(&second_addr).unwrap().push(Xref { from: 0x9000, to: second_addr, kind: XrefKind::Data });
        // The tool's own analysis uses the default limits, so the test caches this program to
        // lower the xref limit without a fixture that needs millions of references.
        let e = engine_with("prog", &tk::elf64(tk::ElfArch::X86_64, false).0);
        e.session().active_mut().unwrap().ext.insert(std::sync::Arc::new(NativeState { img: std::sync::Arc::new(img), prog }));

        let page = |offset: usize| call(&e, "native_strings_find", json!({"pattern": "STATIC_REFERENCE", "offset": offset, "limit": 1}));
        let first_page = page(0);
        assert_eq!(first_page["result"]["total"], 2);
        assert_eq!(first_page["coverage"]["next_offset"], 1);
        let rows = first_page["result"]["matches"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["text"], first);
        assert_eq!(rows[0]["reference_count"], 39, "the analysis kept 39 of the first string's 40 references");
        assert_eq!(rows[0]["references"].as_array().unwrap().len(), MAX_STRING_REFERENCES);
        assert_eq!(rows[0]["references_capped"], true);
        assert!(rows[0]["references"][0]["function"].is_string(), "{}", rows[0]);
        let limits = first_page["limitations"].as_array().unwrap();
        assert!(limits.iter().any(|l| l["code"] == "xref_limit"), "{first_page}");
        let capped = limits.iter().find(|l| l["code"] == "references_capped").expect("the display cap is reported");
        let message = capped["message"].as_str().unwrap();
        assert!(message.contains("analysis xref limit") && !message.contains("complete within this analysis"), "{message}");

        let second_page = page(1);
        assert_eq!(second_page["result"]["total"], 2);
        assert!(second_page["coverage"]["next_offset"].is_null(), "the last page has no next offset");
        let rows = second_page["result"]["matches"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["text"], second);
        assert_eq!(rows[0]["reference_count"], 2, "its kept reference plus one from outside every function");
        assert_eq!(rows[0]["references_capped"], false);
        assert!(rows[0]["references"][1]["function"].is_null(), "{}", rows[0]);
        assert!(
            second_page["limitations"].as_array().unwrap().iter().any(|l| l["code"] == "xref_limit"),
            "the analysis limit reaches a page whose row isn't display-capped: {second_page}"
        );
    }

    #[test]
    fn non_native_targets_get_a_clear_error() {
        let e = engine_with("notes.txt", b"just some text, not a program at all");
        let err = call_err(&e, "native_functions_list", json!({}));
        assert_eq!(err.code, craft_anything_evidence::ErrorCode::NotSupported);
        assert!(err.remediation.is_some());
    }
}
