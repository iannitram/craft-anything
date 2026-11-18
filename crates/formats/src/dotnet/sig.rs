//! Signature blobs (ECMA-335 II.23.2) decoded to C#-like text, and type naming (namespaces,
//! nesting, generic parameter names, type specs). Recursion is bounded by [`MAX_DEPTH`].

use super::meta::{Metadata, compressed_u32, t};
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};

/// Deepest type nesting decoded (arrays of generics of pointers…).
pub const MAX_DEPTH: u32 = 32;
const MAX_PARAMS: u32 = 1024;
const MAX_RANK: u32 = 32;

/// A cursor over a signature blob.
#[derive(Debug, Clone)]
pub struct Cur<'b> {
    d: &'b [u8],
    pub pos: usize,
}

impl<'b> Cur<'b> {
    pub fn new(d: &'b [u8]) -> Self {
        Cur { d, pos: 0 }
    }

    pub fn u8(&mut self) -> Option<u8> {
        let b = *self.d.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    pub fn peek(&self) -> Option<u8> {
        self.d.get(self.pos).copied()
    }

    pub fn cu32(&mut self) -> Option<u32> {
        let (v, n) = compressed_u32(self.d, self.pos)?;
        self.pos += n;
        Some(v)
    }

    /// Compressed signed integer.
    pub fn ci32(&mut self) -> Option<i32> {
        let (v, n) = compressed_u32(self.d, self.pos)?;
        self.pos += n;
        let neg = v & 1 != 0;
        let mag = v >> 1;
        Some(if neg {
            let ext = match n {
                1 => 0xffff_ffc0u32,
                2 => 0xffff_e000,
                _ => 0xf000_0000,
            };
            (mag | ext) as i32
        } else {
            mag as i32
        })
    }

    pub fn rest(&self) -> &'b [u8] {
        self.d.get(self.pos..).unwrap_or(&[])
    }

    pub fn slice(&mut self, n: usize) -> Option<&'b [u8]> {
        let s = self.d.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }

    pub fn u16(&mut self) -> Option<u16> {
        let s = self.slice(2)?;
        Some(u16::from_le_bytes([*s.first()?, *s.get(1)?]))
    }

    pub fn u32(&mut self) -> Option<u32> {
        let s = self.slice(4)?;
        Some(u32::from_le_bytes(s.try_into().ok()?))
    }

    pub fn u64(&mut self) -> Option<u64> {
        let s = self.slice(8)?;
        Some(u64::from_le_bytes(s.try_into().ok()?))
    }
}

/// A decoded method (or property) signature.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct MethodSig {
    pub has_this: bool,
    pub explicit_this: bool,
    pub calling_convention: &'static str,
    pub generic_arity: u32,
    pub return_type: String,
    pub params: Vec<String>,
    /// Index of the first vararg parameter (after the sentinel), if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vararg_from: Option<usize>,
}

impl MethodSig {
    /// `(int, string)`.
    pub fn param_list(&self) -> String {
        format!("({})", self.params.join(", "))
    }
}

/// Generic context: names for `!n` and `!!n`.
#[derive(Debug, Clone, Default)]
pub struct GenCtx<'g> {
    pub type_params: &'g [String],
    pub method_params: &'g [String],
}

/// Member-parent classification of a decoded `TypeSpec` row for `MemberRef` signature binding.
pub(crate) enum TypeSpecParent {
    GenericInst(Vec<String>),
    Array,
    Other,
}

/// Type naming over one metadata set (enclosing types and generic parameter names precomputed).
#[derive(Debug)]
pub struct Names<'m, 'a> {
    pub md: &'m Metadata<'a>,
    enclosing: BTreeMap<u32, u32>,
    type_gp: BTreeMap<u32, Vec<String>>,
    method_gp: BTreeMap<u32, Vec<String>>,
    trailing_signature_data: AtomicBool,
}

/// C# keyword for well-known system types.
pub fn alias(full: &str) -> Option<&'static str> {
    Some(match full {
        "System.Void" => "void",
        "System.Boolean" => "bool",
        "System.Char" => "char",
        "System.SByte" => "sbyte",
        "System.Byte" => "byte",
        "System.Int16" => "short",
        "System.UInt16" => "ushort",
        "System.Int32" => "int",
        "System.UInt32" => "uint",
        "System.Int64" => "long",
        "System.UInt64" => "ulong",
        "System.Single" => "float",
        "System.Double" => "double",
        "System.String" => "string",
        "System.Object" => "object",
        "System.IntPtr" => "nint",
        "System.UIntPtr" => "nuint",
        "System.Decimal" => "decimal",
        _ => return None,
    })
}

/// Split a single type segment (``Outer`1``) into its clean name (`Outer`) and declared generic
/// arity (`Some(1)`), or `(segment, Some(0))` when no numeric backtick suffix is present.
fn segment_arity(segment: &str) -> (&str, Option<usize>) {
    match segment.rfind('`') {
        Some(i) if segment.get(i + 1..).is_some_and(|d| !d.is_empty() && d.bytes().all(|b| b.is_ascii_digit())) => {
            let head = segment.get(..i).unwrap_or(segment);
            let arity = segment.get(i + 1..).and_then(|d| d.parse::<usize>().ok());
            (head, arity)
        }
        _ => (segment, Some(0)),
    }
}

#[cfg(test)]
fn strip_arity(name: &str) -> &str {
    segment_arity(name).0
}

/// Join a CLI `TypeNamespace` and `/`-separated nested type segments into `Namespace.Outer/Inner`.
fn join_ns_parts(ns: &str, parts: &[String]) -> String {
    let joined = parts.join("/");
    if ns.is_empty() { joined } else { format!("{ns}.{joined}") }
}

/// Format a CLI `GENERICINST` (`0x15`) type by distributing `args` across metadata nested-type
/// `parts` according to each segment's backtick arity (``Ns.Outer`1/Inner`` + `[int]` ->
/// `Ns.Outer<int>/Inner`), keeping `ns` untouched and falling back to trailing `<...>` when total
/// arity does not match.
fn format_generic_inst(ns: &str, parts: &[String], args: &[String]) -> String {
    if ns == "System" && parts.len() == 1 && parts.first().map(|p| segment_arity(p)) == Some(("Nullable", Some(1))) && args.len() == 1 {
        return format!("{}?", args.first().map(String::as_str).unwrap_or("?"));
    }
    let segments: Vec<(&str, Option<usize>)> = parts.iter().map(|p| segment_arity(p)).collect();
    let total_arity = segments.iter().try_fold(0usize, |acc, &(_, arity)| acc.checked_add(arity?));
    if total_arity == Some(args.len()) {
        let mut offset = 0usize;
        let mut formatted = Vec::with_capacity(segments.len());
        for (head, arity) in segments {
            let count = arity.unwrap_or(0);
            if count == 0 {
                formatted.push(head.to_string());
            } else {
                let slice = args.get(offset..offset + count).unwrap_or(&[]);
                formatted.push(format!("{head}<{}>", slice.join(", ")));
                offset += count;
            }
        }
        join_ns_parts(ns, &formatted)
    } else {
        let cleaned: Vec<String> = segments.iter().map(|&(head, _)| head.to_string()).collect();
        let full = join_ns_parts(ns, &cleaned);
        let base = if parts.len() == 1 { alias(&full).unwrap_or(&full) } else { &full };
        format!("{base}<{}>", args.join(", "))
    }
}

impl<'m, 'a> Names<'m, 'a> {
    pub fn new(md: &'m Metadata<'a>) -> Self {
        let mut enclosing = BTreeMap::new();
        for r in 1..=md.rows(t::NESTED_CLASS).min(1_000_000) {
            enclosing.insert(md.cell(t::NESTED_CLASS, r, 0), md.cell(t::NESTED_CLASS, r, 1));
        }
        let mut type_gp: BTreeMap<u32, Vec<(u16, String)>> = BTreeMap::new();
        let mut method_gp: BTreeMap<u32, Vec<(u16, String)>> = BTreeMap::new();
        for r in 1..=md.rows(t::GENERIC_PARAM).min(1_000_000) {
            let n = md.cell(t::GENERIC_PARAM, r, 0) as u16;
            let name = md.str_cell(t::GENERIC_PARAM, r, 3);
            match md.coded(t::GENERIC_PARAM, r, 2) {
                Some((t::TYPE_DEF, row)) => type_gp.entry(row).or_default().push((n, name)),
                Some((t::METHOD_DEF, row)) => method_gp.entry(row).or_default().push((n, name)),
                _ => {}
            }
        }
        let fin = |m: BTreeMap<u32, Vec<(u16, String)>>| {
            m.into_iter()
                .map(|(k, mut v)| {
                    v.sort();
                    (k, v.into_iter().map(|(_, n)| n).collect())
                })
                .collect()
        };
        Names { md, enclosing, type_gp: fin(type_gp), method_gp: fin(method_gp), trailing_signature_data: AtomicBool::new(false) }
    }

    /// A bounded warning for complete signatures decoded through this resolver. Padding does
    /// not discard the decoded prefix; incremental types and nested function pointers do not
    /// treat the enclosing signature's remaining bytes as padding.
    pub fn signature_warning(&self) -> Option<&'static str> {
        self.trailing_signature_data
            .load(Ordering::Relaxed)
            .then_some("one or more signature blobs contain trailing bytes; decoded types are retained, but the extra data was not interpreted")
    }

    pub(crate) fn add_signature_warning(&self, warnings: &mut Vec<String>) {
        if let Some(warning) = self.signature_warning() {
            // Keep this single diagnostic visible when a caller caps the displayed warnings.
            warnings.insert(0, warning.into());
        }
    }

    pub(crate) fn finish_signature<T>(&self, cursor: &Cur<'_>, decoded: T) -> T {
        if !cursor.rest().is_empty() {
            self.trailing_signature_data.store(true, Ordering::Relaxed);
        }
        decoded
    }

    pub fn type_generics(&self, typedef: u32) -> &[String] {
        self.type_gp.get(&typedef).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn method_generics(&self, method: u32) -> &[String] {
        self.method_gp.get(&method).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn enclosing(&self, typedef: u32) -> Option<u32> {
        self.enclosing.get(&typedef).copied()
    }

    /// Resolve a `TypeDef` row into `(namespace, nested_segments)` without merging `/` characters.
    fn typedef_parts(&self, row: u32) -> (String, Vec<String>) {
        let mut parts = Vec::new();
        let mut cur = row;
        let mut ns = String::new();
        for _ in 0..16 {
            parts.push(self.md.str_cell(t::TYPE_DEF, cur, 1));
            match self.enclosing(cur) {
                Some(outer) if outer != cur && outer != 0 => cur = outer,
                _ => {
                    ns = self.md.str_cell(t::TYPE_DEF, cur, 2);
                    break;
                }
            }
        }
        parts.reverse();
        (ns, parts)
    }

    /// `Namespace.Outer/Inner` (metadata names, arity suffixes kept).
    pub fn typedef_name(&self, row: u32) -> String {
        let (ns, parts) = self.typedef_parts(row);
        join_ns_parts(&ns, &parts)
    }

    /// Resolve a `TypeRef` row into `(namespace, nested_segments)` without merging `/` characters.
    fn typeref_parts(&self, row: u32) -> (String, Vec<String>) {
        let mut parts = Vec::new();
        let mut cur = row;
        let mut ns = String::new();
        for _ in 0..16 {
            parts.push(self.md.str_cell(t::TYPE_REF, cur, 1));
            match self.md.coded(t::TYPE_REF, cur, 0) {
                Some((t::TYPE_REF, outer)) if outer != cur => cur = outer,
                _ => {
                    ns = self.md.str_cell(t::TYPE_REF, cur, 2);
                    break;
                }
            }
        }
        parts.reverse();
        (ns, parts)
    }

    pub fn typeref_name(&self, row: u32) -> String {
        let (ns, parts) = self.typeref_parts(row);
        join_ns_parts(&ns, &parts)
    }

    /// Where a TypeRef resolves (assembly or module name), when known.
    pub fn typeref_scope(&self, row: u32) -> Option<String> {
        let mut cur = row;
        for _ in 0..16 {
            match self.md.coded(t::TYPE_REF, cur, 0)? {
                (t::TYPE_REF, outer) => cur = outer,
                (t::ASSEMBLY_REF, a) => return Some(self.md.str_cell(t::ASSEMBLY_REF, a, 6)),
                (t::MODULE_REF, m) => return Some(self.md.str_cell(t::MODULE_REF, m, 0)),
                (t::MODULE, _) => return Some(self.md.str_cell(t::MODULE, 1, 1)),
                _ => return None,
            }
        }
        None
    }

    /// Display name of a TypeDef, TypeRef or TypeSpec (C# keywords for system types).
    pub fn type_name(&self, table: u8, row: u32, g: &GenCtx<'_>, depth: u32) -> String {
        match table {
            t::TYPE_DEF => {
                let n = self.typedef_name(row);
                alias(&n).map(String::from).unwrap_or(n)
            }
            t::TYPE_REF => {
                let n = self.typeref_name(row);
                alias(&n).map(String::from).unwrap_or(n)
            }
            t::TYPE_SPEC => {
                let blob = self.md.blob_cell(t::TYPE_SPEC, row, 0);
                let mut c = Cur::new(blob);
                self.ty(&mut c, g, depth + 1).map(|ty| self.finish_signature(&c, ty)).unwrap_or_else(|| "?".into())
            }
            _ => "?".into(),
        }
    }

    /// Decode a `TypeSpec` row in `g`, returning both its formatted display name and its
    /// `MemberRefParent` classification (`GENERICINST` type arguments or array pseudo-method
    /// parent) under the same modifier and depth rules as [`Self::type_name`].
    pub(crate) fn typespec_parent(&self, row: u32, g: &GenCtx<'_>) -> (String, TypeSpecParent) {
        let blob = self.md.blob_cell(t::TYPE_SPEC, row, 0);
        let mut probe = Cur::new(blob);
        let head = self.skip_mods(&mut probe, g, 1).and_then(|is_volatile| Some((is_volatile, probe.u8()?)));
        if let Some((is_volatile, 0x15)) = head {
            return match self.generic_inst_body(&mut probe, g, 2) {
                Some((formatted, args)) => {
                    let display = if is_volatile { format!("volatile {formatted}") } else { formatted };
                    (self.finish_signature(&probe, display), TypeSpecParent::GenericInst(args))
                }
                None => ("?".into(), TypeSpecParent::Other),
            };
        }
        let mut c = Cur::new(blob);
        match self.ty(&mut c, g, 1) {
            Some(display) => {
                let kind = if matches!(head, Some((_, 0x14 | 0x1d))) { TypeSpecParent::Array } else { TypeSpecParent::Other };
                (self.finish_signature(&c, display), kind)
            }
            None => ("?".into(), TypeSpecParent::Other),
        }
    }

    /// Parse the `(CLASS | VALUETYPE) TypeDefOrRef n args` tail of a `GENERICINST` (`0x15`) element.
    fn generic_inst_body(&self, c: &mut Cur<'_>, g: &GenCtx<'_>, arg_depth: u32) -> Option<(String, Vec<String>)> {
        let kind = c.u8()?;
        if kind != 0x11 && kind != 0x12 {
            return None;
        }
        let v = c.cu32()?;
        let (ns, parts) = match v & 3 {
            0 => self.typedef_parts(v >> 2),
            1 => self.typeref_parts(v >> 2),
            _ => return None,
        };
        let n = c.cu32()?;
        if n == 0 || n > 64 {
            return None;
        }
        let mut args = Vec::with_capacity(n as usize);
        for _ in 0..n {
            args.push(self.ty(c, g, arg_depth)?);
        }
        Some((format_generic_inst(&ns, &parts, &args), args))
    }

    fn tdor_encoded(&self, c: &mut Cur<'_>, g: &GenCtx<'_>, depth: u32, allow_type_spec: bool) -> Option<String> {
        let v = c.cu32()?;
        let table = match v & 3 {
            0 => t::TYPE_DEF,
            1 => t::TYPE_REF,
            2 if allow_type_spec => t::TYPE_SPEC,
            _ => return None,
        };
        Some(self.type_name(table, v >> 2, g, depth))
    }

    fn skip_mods(&self, c: &mut Cur<'_>, g: &GenCtx<'_>, depth: u32) -> Option<bool> {
        let mut is_volatile = false;
        while matches!(c.peek(), Some(0x1f | 0x20)) {
            let req = c.u8()? == 0x1f;
            let m = self.tdor_encoded(c, g, depth, true)?;
            is_volatile |= req && m == "System.Runtime.CompilerServices.IsVolatile";
        }
        Some(is_volatile)
    }

    /// One type from a signature.
    pub fn ty(&self, c: &mut Cur<'_>, g: &GenCtx<'_>, depth: u32) -> Option<String> {
        if depth > MAX_DEPTH {
            return None;
        }
        let is_volatile = self.skip_mods(c, g, depth)?;
        let b = c.u8()?;
        let s = match b {
            0x01 => "void".into(),
            0x02 => "bool".into(),
            0x03 => "char".into(),
            0x04 => "sbyte".into(),
            0x05 => "byte".into(),
            0x06 => "short".into(),
            0x07 => "ushort".into(),
            0x08 => "int".into(),
            0x09 => "uint".into(),
            0x0a => "long".into(),
            0x0b => "ulong".into(),
            0x0c => "float".into(),
            0x0d => "double".into(),
            0x0e => "string".into(),
            0x0f => format!("{}*", self.ty(c, g, depth + 1)?),
            0x10 => format!("ref {}", self.ty(c, g, depth + 1)?),
            // CLI CLASS and VALUETYPE name definitions or references, never another signature.
            0x11 | 0x12 => self.tdor_encoded(c, g, depth + 1, false)?,
            0x13 => {
                let n = c.cu32()?;
                g.type_params.get(n as usize).cloned().unwrap_or_else(|| format!("!{n}"))
            }
            0x1e => {
                let n = c.cu32()?;
                g.method_params.get(n as usize).cloned().unwrap_or_else(|| format!("!!{n}"))
            }
            0x14 => {
                let el = self.ty(c, g, depth + 1)?;
                let rank = c.cu32()?;
                if rank == 0 || rank > MAX_RANK {
                    return None;
                }
                let nsizes = c.cu32()?;
                if nsizes > rank {
                    return None;
                }
                let sizes = (0..nsizes).map(|_| c.cu32()).collect::<Option<Vec<_>>>()?;
                let nlo = c.cu32()?;
                if nlo > rank {
                    return None;
                }
                let lower_bounds = (0..nlo).map(|_| c.ci32()).collect::<Option<Vec<_>>>()?;
                // General ARRAY is not SZARRAY, even at rank one. Keep its CLI shape
                // notation, including any sizes and signed lower bounds from metadata.
                let dimensions = (0..rank as usize)
                    .map(|dimension| match (sizes.get(dimension), lower_bounds.get(dimension)) {
                        (Some(&size), lower) => {
                            let lower = i64::from(lower.copied().unwrap_or(0));
                            let upper = lower + i64::from(size) - 1;
                            format!("{lower}...{upper}")
                        }
                        (None, Some(lower)) => format!("{lower}..."),
                        (None, None) if rank == 1 => "*".into(),
                        (None, None) => String::new(),
                    })
                    .collect::<Vec<_>>();
                format!("{el}[{}]", dimensions.join(","))
            }
            0x15 => self.generic_inst_body(c, g, depth + 1)?.0,
            0x16 => "TypedReference".into(),
            0x18 => "nint".into(),
            0x19 => "nuint".into(),
            0x1b => {
                let m = self.method_sig_inner(c, g, None, depth + 1)?;
                let mut parts = m.params.clone();
                parts.push(m.return_type.clone());
                if m.calling_convention == "default" {
                    format!("delegate*<{}>", parts.join(", "))
                } else {
                    format!("delegate* unmanaged[{}]<{}>", m.calling_convention, parts.join(", "))
                }
            }
            0x1c => "object".into(),
            0x1d => format!("{}[]", self.ty(c, g, depth + 1)?),
            0x45 => format!("pinned {}", self.ty(c, g, depth + 1)?),
            _ => return None,
        };
        Some(if is_volatile { format!("volatile {s}") } else { s })
    }

    fn method_sig_inner(&self, c: &mut Cur<'_>, g: &GenCtx<'_>, vararg_g: Option<&GenCtx<'_>>, depth: u32) -> Option<MethodSig> {
        if depth > MAX_DEPTH {
            return None;
        }
        let flags = c.u8()?;
        let generic_arity = if flags & 0x10 != 0 { c.cu32()? } else { 0 };
        let count = c.cu32()?;
        if count > MAX_PARAMS || generic_arity > 1024 {
            return None;
        }
        let return_type = self.ty(c, g, depth + 1)?;
        let mut params = Vec::new();
        let mut vararg_from = None;
        let is_vararg = (flags & 0x0f) == 5;
        for _ in 0..count {
            if c.peek() == Some(0x41) {
                c.u8()?;
                vararg_from = Some(params.len());
            }
            let param_g = match (is_vararg && vararg_from.is_some(), vararg_g) {
                (true, Some(vg)) => vg,
                _ => g,
            };
            params.push(self.ty(c, param_g, depth + 1)?);
        }
        let calling_convention = match flags & 0x0f {
            0 => "default",
            1 => "cdecl",
            2 => "stdcall",
            3 => "thiscall",
            4 => "fastcall",
            5 => "vararg",
            6 => "field",
            7 => "local",
            8 => "property",
            9 => "unmanaged",
            0x0a => "generic-instance",
            _ => "unknown",
        };
        Some(MethodSig { has_this: flags & 0x20 != 0, explicit_this: flags & 0x40 != 0, calling_convention, generic_arity, return_type, params, vararg_from })
    }

    /// A MethodDefSig, MethodRefSig or PropertySig.
    pub fn method_sig(&self, blob: &[u8], g: &GenCtx<'_>) -> Option<MethodSig> {
        let mut c = Cur::new(blob);
        let signature = self.method_sig_inner(&mut c, g, None, 0)?;
        Some(self.finish_signature(&c, signature))
    }

    /// A `MethodRefSig` whose fixed parameters bind to `g` and whose post-`0x41` (`SENTINEL`)
    /// `vararg` call-site arguments bind to `caller_g`.
    pub(crate) fn method_ref_sig(&self, blob: &[u8], g: &GenCtx<'_>, caller_g: &GenCtx<'_>) -> Option<MethodSig> {
        let mut c = Cur::new(blob);
        let signature = self.method_sig_inner(&mut c, g, Some(caller_g), 0)?;
        Some(self.finish_signature(&c, signature))
    }

    /// A FieldSig (`0x06 type`).
    pub fn field_sig(&self, blob: &[u8], g: &GenCtx<'_>) -> Option<String> {
        let mut c = Cur::new(blob);
        if c.u8()? != 0x06 {
            return None;
        }
        let field_type = self.ty(&mut c, g, 0)?;
        Some(self.finish_signature(&c, field_type))
    }

    /// A LocalVarSig (`0x07 count types`).
    pub fn locals_sig(&self, blob: &[u8], g: &GenCtx<'_>) -> Option<Vec<String>> {
        let mut c = Cur::new(blob);
        if c.u8()? != 0x07 {
            return None;
        }
        let n = c.cu32()?;
        if n > 65_535 {
            return None;
        }
        let mut v = Vec::new();
        for _ in 0..n {
            if c.peek() == Some(0x16) {
                c.u8()?;
                v.push("TypedReference".into());
                continue;
            }
            v.push(self.ty(&mut c, g, 0)?);
        }
        Some(self.finish_signature(&c, v))
    }

    /// A MethodSpec instantiation (`0x0a count types`).
    pub fn method_spec_args(&self, blob: &[u8], g: &GenCtx<'_>) -> Option<Vec<String>> {
        let mut c = Cur::new(blob);
        if c.u8()? != 0x0a {
            return None;
        }
        let n = c.cu32()?;
        if n == 0 || n > 64 {
            return None;
        }
        let mut v = Vec::new();
        for _ in 0..n {
            v.push(self.ty(&mut c, g, 0)?);
        }
        Some(self.finish_signature(&c, v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_compression() {
        assert_eq!(Cur::new(&[0x06]).ci32(), Some(3));
        assert_eq!(Cur::new(&[0x7b]).ci32(), Some(-3));
        assert_eq!(Cur::new(&[0x80, 0x01]).ci32(), Some(-8192));
        assert_eq!(strip_arity("List`1"), "List");
        assert_eq!(strip_arity("A`b"), "A`b");
    }
}
