use std::collections::BTreeMap;

use craft_anything_formats::dotnet::{
    self, MemberOptions, csharp,
    meta::{Col, schema, t},
};

/// Encode an unsigned integer using ECMA-335 compressed unsigned integer format.
fn compressed_u32(value: usize) -> Vec<u8> {
    if value < 0x80 {
        vec![value as u8]
    } else if value < 0x4000 {
        vec![0x80 | (value >> 8) as u8, value as u8]
    } else {
        vec![0xc0 | (value >> 24) as u8, (value >> 16) as u8, (value >> 8) as u8, value as u8]
    }
}

/// Pad a byte buffer with zeroes to a 4-byte boundary.
fn pad4(buf: &mut Vec<u8>) {
    while !buf.len().is_multiple_of(4) {
        buf.push(0);
    }
}

/// Wrap raw CIL bytes in a fat method body header so bodies of any length (`>= 64` bytes) are valid.
fn fat_body(code: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(12 + code.len());
    // Fat format: flags = 0x3003 (size = 3 dwords = 12 bytes, format = 3), max_stack = 8.
    out.extend_from_slice(&0x3003u16.to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&(code.len() as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // local_var_sig_tok = 0
    out.extend_from_slice(code);
    out
}

/// String and blob heap builder for synthetic CLI assemblies.
struct Heaps {
    strings: Vec<u8>,
    blob: Vec<u8>,
    us: Vec<u8>,
    guid: Vec<u8>,
}

impl Heaps {
    /// Create heaps initialized with leading empty entries and one dummy GUID.
    fn new() -> Self {
        Self { strings: vec![0], blob: vec![0], us: vec![0], guid: vec![0x11; 16] }
    }

    /// Intern a NUL-terminated UTF-8 string into `#Strings` and return its 16-bit offset.
    fn str(&mut self, value: &str) -> u32 {
        if value.is_empty() {
            return 0;
        }
        let offset = self.strings.len() as u32;
        self.strings.extend_from_slice(value.as_bytes());
        self.strings.push(0);
        offset
    }

    /// Intern a length-prefixed signature or instantiation into `#Blob` and return its 16-bit offset.
    fn blob(&mut self, bytes: &[u8]) -> u32 {
        if bytes.is_empty() {
            return 0;
        }
        let offset = self.blob.len() as u32;
        self.blob.extend(compressed_u32(bytes.len()));
        self.blob.extend_from_slice(bytes);
        offset
    }
}

/// Encode `(table, row)` as a 2-bit `TypeDefOrRef` coded index.
const fn tdor(table: u8, row: u32) -> u32 {
    let tag = match table {
        t::TYPE_DEF => 0,
        t::TYPE_REF => 1,
        t::TYPE_SPEC => 2,
        _ => panic!("unsupported TypeDefOrRef table"),
    };
    (row << 2) | tag
}

/// Encode `(table, row)` as a 3-bit `MemberRefParent` coded index.
const fn mrp(table: u8, row: u32) -> u32 {
    let tag = match table {
        t::TYPE_DEF => 0,
        t::TYPE_REF => 1,
        t::MODULE_REF => 2,
        t::METHOD_DEF => 3,
        t::TYPE_SPEC => 4,
        _ => panic!("unsupported MemberRefParent table"),
    };
    (row << 3) | tag
}

/// Encode `(table, row)` as a 1-bit `MethodDefOrRef` coded index.
const fn mdor(table: u8, row: u32) -> u32 {
    let tag = match table {
        t::METHOD_DEF => 0,
        t::MEMBER_REF => 1,
        _ => panic!("unsupported MethodDefOrRef table"),
    };
    (row << 1) | tag
}

/// Encode `(table, row)` as a 1-bit `TypeOrMethodDef` coded index.
const fn tomd(table: u8, row: u32) -> u32 {
    let tag = match table {
        t::TYPE_DEF => 0,
        t::METHOD_DEF => 1,
        _ => panic!("unsupported TypeOrMethodDef table"),
    };
    (row << 1) | tag
}

/// Encode `(table, row)` as a 32-bit metadata token in little-endian bytes for CIL operands.
const fn tok_le(table: u8, row: u32) -> [u8; 4] {
    (((table as u32) << 24) | row).to_le_bytes()
}

/// Build a synthetic PE32 CLI assembly exercising generic events, `MemberRef` calls/fields,
/// `MethodSpec` instantiations, `vararg` call-site arguments, array pseudo-methods, `TypeDef`
/// parents, `modopt`-prefixed and malformed/deep `TypeSpec` parents, and `TypeSpec` IL operands.
fn build_generic_context_assembly() -> Vec<u8> {
    const TEXT_RVA: u32 = 0x2000;
    let mut h = Heaps::new();
    let mut tables: BTreeMap<u8, Vec<Vec<u32>>> = BTreeMap::new();

    tables.insert(t::MODULE, vec![vec![0, h.str("GenericCtx.dll"), 1, 0, 0]]);
    tables.insert(t::ASSEMBLY, vec![vec![0x8004, 1, 0, 0, 0, 0, 0, h.str("GenericCtx"), 0]]);
    tables.insert(t::ASSEMBLY_REF, vec![vec![8, 0, 0, 0, 0, 0, h.str("System.Runtime"), 0, 0]]);

    // TypeRef (0x01):
    // 1: System.Object
    // 2: System.EventHandler
    // 3: System.Action`2
    // 4: System.Action`1
    // 5: System.Collections.Generic.Dictionary`2
    // 6: System.Collections.Generic.List`1
    // 7: Ns.Holder`1
    // 8: Ext.PlainHelper (non-generic external type)
    let asm_ref_1 = (1 << 2) | 2;
    tables.insert(
        t::TYPE_REF,
        vec![
            vec![asm_ref_1, h.str("Object"), h.str("System")],
            vec![asm_ref_1, h.str("EventHandler"), h.str("System")],
            vec![asm_ref_1, h.str("Action`2"), h.str("System")],
            vec![asm_ref_1, h.str("Action`1"), h.str("System")],
            vec![asm_ref_1, h.str("Dictionary`2"), h.str("System.Collections.Generic")],
            vec![asm_ref_1, h.str("List`1"), h.str("System.Collections.Generic")],
            vec![asm_ref_1, h.str("Holder`1"), h.str("Ns")],
            vec![asm_ref_1, h.str("PlainHelper"), h.str("Ext")],
        ],
    );

    // Build a TypeSpec with 65 leading `modopt(Ext.PlainHelper)` custom modifiers before `List<!1>`.
    let mut modopt_list = Vec::new();
    for _ in 0..65 {
        modopt_list.push(0x20);
        modopt_list.push(tdor(t::TYPE_REF, 8) as u8);
    }
    modopt_list.extend_from_slice(&[0x15, 0x12, tdor(t::TYPE_REF, 6) as u8, 1, 0x13, 1]);

    // Build an over-deep TypeSpec (`List<!1[][][...(31x)...]>`) that `Names::type_name` rejects as `"?"`.
    let mut overdeep_list = vec![0x15, 0x12, tdor(t::TYPE_REF, 6) as u8, 1];
    overdeep_list.extend(std::iter::repeat_n(0x1du8, 31));
    overdeep_list.extend_from_slice(&[0x13, 1]);

    // TypeSpec (0x1b):
    // 1:  System.Action<!0, !1>
    // 2:  System.Action<!1>
    // 3:  Dictionary<!1, !!0>
    // 4:  Ns.Holder<!1>
    // 5:  List<int>
    // 6:  !1                                 (for `box !1` -> `box TVal`)
    // 7:  !!0                                (for `newarr !!0` & `ldtoken !!0` -> `TMsg`)
    // 8:  !!0[,]                             (2D general array `TMsg[,]`)
    // 9:  !1[]                               (1D SZARRAY `TVal[]`)
    // 10: 65x modopt(Ext.PlainHelper) List<!1>
    // 11: truncated SZARRAY `[0x1d]`         (malformed -> `"?"`)
    // 12: over-deep `List<TVal[]*31>`        (exceeds MAX_DEPTH -> `"?"`)
    tables.insert(
        t::TYPE_SPEC,
        vec![
            vec![h.blob(&[0x15, 0x12, tdor(t::TYPE_REF, 3) as u8, 2, 0x13, 0, 0x13, 1])],
            vec![h.blob(&[0x15, 0x12, tdor(t::TYPE_REF, 4) as u8, 1, 0x13, 1])],
            vec![h.blob(&[0x15, 0x12, tdor(t::TYPE_REF, 5) as u8, 2, 0x13, 1, 0x1e, 0])],
            vec![h.blob(&[0x15, 0x12, tdor(t::TYPE_REF, 7) as u8, 1, 0x13, 1])],
            vec![h.blob(&[0x15, 0x12, tdor(t::TYPE_REF, 6) as u8, 1, 0x08])],
            vec![h.blob(&[0x13, 1])],
            vec![h.blob(&[0x1e, 0])],
            vec![h.blob(&[0x14, 0x1e, 0, 2, 0, 0])],
            vec![h.blob(&[0x1d, 0x13, 1])],
            vec![h.blob(&modopt_list)],
            vec![h.blob(&[0x1d])],
            vec![h.blob(&overdeep_list)],
        ],
    );

    // MemberRef (0x0a):
    // 1:  Dictionary<!1, !!0>::Add(!0, !1)
    // 2:  Ns.Holder<!1>::Item (field)
    // 3:  Ns.Holder<!1>::Convert<1>(!0, !!0)
    // 4:  TMsg[,]::Set(int, int, !!0)
    // 5:  List<int>::Add(!0)
    // 6:  Ext.PlainHelper::Ping(int)
    // 7:  Ext.PlainHelper::Raw(!0, !!0)                   (unbound fixed params on non-generic TypeRef stay `!0, !!0`)
    // 8:  TVal[]::Set(int, !1)                            (1D SZARRAY pseudo-method referencing caller `!1`)
    // 9:  65x-modopt List<!1>::Add(!0)                    (custom modifiers before GENERICINST)
    // 10: TypeDef(6) (`Ns.LocalGen`1<TItem>`)::Take(!0)   (MemberRef parent = TypeDef)
    // 11: Ext.PlainHelper::LogVararg(!0, ..., !0, !1, !!0)(vararg `0x05`: fixed `!0` stays `!0`, post-`0x41` resolves caller)
    // 12: Ns.Holder<!1>::NonGen(!!0)                      (non-generic flag `0x20` wrapped in malformed MethodSpec -> stays `!!0`)
    // 13: truncated TypeSpec(11) `BadSz(!0, !!0)`         (broken parent `?` -> stays `?::BadSz(!0, !!0)`)
    // 14: over-deep TypeSpec(12) `DeepAdd(!0)`            (broken parent `?` -> stays `?::DeepAdd(!0)`)
    tables.insert(
        t::MEMBER_REF,
        vec![
            vec![mrp(t::TYPE_SPEC, 3), h.str("Add"), h.blob(&[0x20, 2, 0x01, 0x13, 0, 0x13, 1])],
            vec![mrp(t::TYPE_SPEC, 4), h.str("Item"), h.blob(&[0x06, 0x13, 0])],
            vec![mrp(t::TYPE_SPEC, 4), h.str("Convert"), h.blob(&[0x30, 1, 2, 0x1e, 0, 0x13, 0, 0x1e, 0])],
            vec![mrp(t::TYPE_SPEC, 8), h.str("Set"), h.blob(&[0x20, 3, 0x01, 0x08, 0x08, 0x1e, 0])],
            vec![mrp(t::TYPE_SPEC, 5), h.str("Add"), h.blob(&[0x20, 1, 0x01, 0x13, 0])],
            vec![mrp(t::TYPE_REF, 8), h.str("Ping"), h.blob(&[0x00, 1, 0x01, 0x08])],
            vec![mrp(t::TYPE_REF, 8), h.str("Raw"), h.blob(&[0x00, 2, 0x01, 0x13, 0, 0x1e, 0])],
            vec![mrp(t::TYPE_SPEC, 9), h.str("Set"), h.blob(&[0x20, 2, 0x01, 0x08, 0x13, 1])],
            vec![mrp(t::TYPE_SPEC, 10), h.str("Add"), h.blob(&[0x20, 1, 0x01, 0x13, 0])],
            vec![mrp(t::TYPE_DEF, 6), h.str("Take"), h.blob(&[0x20, 1, 0x01, 0x13, 0])],
            vec![mrp(t::TYPE_REF, 8), h.str("LogVararg"), h.blob(&[0x05, 4, 0x01, 0x13, 0, 0x41, 0x13, 0, 0x13, 1, 0x1e, 0])],
            vec![mrp(t::TYPE_SPEC, 4), h.str("NonGen"), h.blob(&[0x20, 1, 0x01, 0x1e, 0])],
            vec![mrp(t::TYPE_SPEC, 11), h.str("BadSz"), h.blob(&[0x20, 2, 0x01, 0x13, 0, 0x1e, 0])],
            vec![mrp(t::TYPE_SPEC, 12), h.str("DeepAdd"), h.blob(&[0x20, 1, 0x01, 0x13, 0])],
        ],
    );

    // MethodSpec (0x2b):
    // 1: MemberRef(3) instantiated with [!!0] -> `Ns.Holder<TVal>::Convert(TVal, TMsg)<TMsg>`
    // 2: MethodDef(1) (`Ns.Util::Identity<U>`) instantiated with [!!0] -> `Ns.Util::Identity(U)<TMsg>`
    // 3: Malformed MethodSpec wrapping non-generic MemberRef(12) with [!!0] -> `Ns.Holder<TVal>::NonGen(!!0)<TMsg>`
    tables.insert(
        t::METHOD_SPEC,
        vec![
            vec![mdor(t::MEMBER_REF, 3), h.blob(&[0x0a, 1, 0x1e, 0])],
            vec![mdor(t::METHOD_DEF, 1), h.blob(&[0x0a, 1, 0x1e, 0])],
            vec![mdor(t::MEMBER_REF, 12), h.blob(&[0x0a, 1, 0x1e, 0])],
        ],
    );

    let mut bodies = Vec::new();
    let mut push_body = |code: &[u8]| -> u32 {
        let rva = TEXT_RVA + 72 + bodies.len() as u32;
        bodies.extend(fat_body(code));
        pad4(&mut bodies);
        rva
    };

    // MethodDef 1: `Ns.Util::Identity<U>(U x)`
    let rva_identity = push_body(&[0x02, 0x2a]);

    // MethodDef 2: `Ns.Plain::Run()`
    let mut plain_il = Vec::new();
    plain_il.push(0x28);
    plain_il.extend_from_slice(&tok_le(t::MEMBER_REF, 6));
    plain_il.push(0x6f);
    plain_il.extend_from_slice(&tok_le(t::MEMBER_REF, 5));
    plain_il.push(0x2a);
    let rva_plain = push_body(&plain_il);

    // MethodDef 3: `Ns.Bus`2::Dispatch<TMsg>(TVal val, TMsg msg)`
    let mut dispatch_il = Vec::new();
    for (op, tbl, row) in [
        (0x8c, t::TYPE_SPEC, 6),   // 0: box TVal
        (0x8d, t::TYPE_SPEC, 7),   // 1: newarr TMsg
        (0xd0, t::TYPE_SPEC, 7),   // 2: ldtoken TMsg
        (0x6f, t::MEMBER_REF, 1),  // call 0: Dictionary<TVal, TMsg>::Add(TVal, TMsg)
        (0x7d, t::MEMBER_REF, 2),  // field 0: Ns.Holder<TVal>::Item
        (0x6f, t::METHOD_SPEC, 1), // call 1: Ns.Holder<TVal>::Convert(TVal, TMsg)<TMsg>
        (0x28, t::METHOD_SPEC, 2), // call 2: Ns.Util::Identity(U)<TMsg>
        (0x28, t::MEMBER_REF, 4),  // call 3: TMsg[,]::Set(int, int, TMsg)
        (0x28, t::MEMBER_REF, 7),  // call 4: Ext.PlainHelper::Raw(!0, !!0)
        (0x28, t::MEMBER_REF, 8),  // call 5: TVal[]::Set(int, TVal)
        (0x6f, t::MEMBER_REF, 9),  // call 6: System.Collections.Generic.List<TVal>::Add(TVal) (65x modopt)
        (0x6f, t::MEMBER_REF, 10), // call 7: Ns.LocalGen`1::Take(TItem) (TypeDef parent)
        (0x28, t::MEMBER_REF, 11), // call 8: Ext.PlainHelper::LogVararg(!0, TKey, TVal, TMsg)
        (0x6f, t::METHOD_SPEC, 3), // call 9: Ns.Holder<TVal>::NonGen(!!0)<TMsg>
        (0x28, t::MEMBER_REF, 13), // call 10: ?::BadSz(!0, !!0)
        (0x6f, t::MEMBER_REF, 14), // call 11: ?::DeepAdd(!0)
    ] {
        dispatch_il.push(op);
        dispatch_il.extend_from_slice(&tok_le(tbl, row));
    }
    dispatch_il.push(0x2a);
    let rva_dispatch = push_body(&dispatch_il);

    // TypeDef (0x02):
    // 1: <Module>
    // 2: Ns.Util (MethodDef 1..2)
    // 3: Ns.Plain (MethodDef 2..3, Event 1)
    // 4: Ns.Bus`2 (MethodDef 3..4, Event 2)
    // 5: Nested under Ns.Bus`2 (MethodDef 4..4, Event 3)
    // 6: Ns.LocalGen`1 (MethodDef 4..4, generic param TItem)
    let obj = tdor(t::TYPE_REF, 1);
    tables.insert(
        t::TYPE_DEF,
        vec![
            vec![0, h.str("<Module>"), 0, 0, 1, 1],
            vec![0x0010_0001, h.str("Util"), h.str("Ns"), obj, 1, 1],
            vec![0x0010_0001, h.str("Plain"), h.str("Ns"), obj, 1, 2],
            vec![0x0010_0001, h.str("Bus`2"), h.str("Ns"), obj, 1, 3],
            vec![0x0010_0002, h.str("Nested"), 0, obj, 1, 4],
            vec![0x0010_0001, h.str("LocalGen`1"), h.str("Ns"), obj, 1, 4],
        ],
    );
    tables.insert(t::NESTED_CLASS, vec![vec![5, 4]]);

    tables.insert(
        t::METHOD_DEF,
        vec![
            vec![rva_identity, 0, 0x0016, h.str("Identity"), h.blob(&[0x10, 1, 1, 0x1e, 0, 0x1e, 0]), 1],
            vec![rva_plain, 0, 0x0006, h.str("Run"), h.blob(&[0x20, 0, 0x01]), 1],
            vec![rva_dispatch, 0, 0x0006, h.str("Dispatch"), h.blob(&[0x30, 1, 2, 0x1e, 0, 0x13, 1, 0x1e, 0]), 1],
        ],
    );

    tables.insert(t::EVENT_MAP, vec![vec![3, 1], vec![4, 2], vec![5, 3]]);
    tables.insert(
        t::EVENT,
        vec![
            vec![0, h.str("Ready"), tdor(t::TYPE_REF, 2)],
            vec![0, h.str("Fired"), tdor(t::TYPE_SPEC, 1)],
            vec![0, h.str("NestedFired"), tdor(t::TYPE_SPEC, 2)],
        ],
    );

    tables.insert(
        t::GENERIC_PARAM,
        vec![
            vec![0, 0, tomd(t::TYPE_DEF, 4), h.str("TKey")],
            vec![1, 0, tomd(t::TYPE_DEF, 4), h.str("TVal")],
            vec![0, 0, tomd(t::TYPE_DEF, 5), h.str("TKey")],
            vec![1, 0, tomd(t::TYPE_DEF, 5), h.str("TVal")],
            vec![0, 0, tomd(t::TYPE_DEF, 6), h.str("TItem")],
            vec![0, 0, tomd(t::METHOD_DEF, 1), h.str("U")],
            vec![0, 0, tomd(t::METHOD_DEF, 3), h.str("TMsg")],
        ],
    );

    let mut valid = 0u64;
    for &table in tables.keys() {
        valid |= 1u64 << table;
    }
    let mut ts = Vec::new();
    ts.extend_from_slice(&0u32.to_le_bytes());
    ts.push(2);
    ts.push(0);
    ts.push(0);
    ts.push(1);
    ts.extend_from_slice(&valid.to_le_bytes());
    ts.extend_from_slice(&0u64.to_le_bytes());
    for rows in tables.values() {
        ts.extend_from_slice(&(rows.len() as u32).to_le_bytes());
    }
    for (&table, rows) in &tables {
        let cols = schema(table);
        for row in rows {
            assert_eq!(row.len(), cols.len(), "table {table:#x}");
            for (&val, &col) in row.iter().zip(cols) {
                if matches!(col, Col::U32) {
                    ts.extend_from_slice(&val.to_le_bytes());
                } else {
                    ts.extend_from_slice(&(val as u16).to_le_bytes());
                }
            }
        }
    }
    pad4(&mut ts);
    for heap in [&mut h.strings, &mut h.blob, &mut h.us, &mut h.guid] {
        pad4(heap);
    }

    let streams: [(&str, &[u8]); 5] = [("#~", &ts), ("#Strings", &h.strings), ("#US", &h.us), ("#GUID", &h.guid), ("#Blob", &h.blob)];
    let mut header_len = 16 + 12 + 4;
    for (name, _) in &streams {
        header_len += 8 + (name.len() + 1).div_ceil(4) * 4;
    }
    let mut md = Vec::new();
    md.extend_from_slice(b"BSJB");
    md.extend_from_slice(&1u16.to_le_bytes());
    md.extend_from_slice(&1u16.to_le_bytes());
    md.extend_from_slice(&0u32.to_le_bytes());
    md.extend_from_slice(&12u32.to_le_bytes());
    md.extend_from_slice(b"v4.0.30319\0\0");
    md.extend_from_slice(&0u16.to_le_bytes());
    md.extend_from_slice(&(streams.len() as u16).to_le_bytes());
    let mut off = header_len;
    for (name, data) in &streams {
        md.extend_from_slice(&(off as u32).to_le_bytes());
        md.extend_from_slice(&(data.len() as u32).to_le_bytes());
        let mut nb = name.as_bytes().to_vec();
        nb.push(0);
        pad4(&mut nb);
        md.extend(nb);
        off += data.len();
    }
    for (_, data) in &streams {
        md.extend_from_slice(data);
    }

    let md_rva = TEXT_RVA + 72 + bodies.len() as u32;
    let mut text = Vec::new();
    text.extend_from_slice(&72u32.to_le_bytes());
    text.extend_from_slice(&2u16.to_le_bytes());
    text.extend_from_slice(&5u16.to_le_bytes());
    text.extend_from_slice(&md_rva.to_le_bytes());
    text.extend_from_slice(&(md.len() as u32).to_le_bytes());
    text.extend_from_slice(&1u32.to_le_bytes());
    text.resize(72, 0);
    text.extend(bodies);
    text.extend(md);
    let raw_size = text.len().div_ceil(0x200) * 0x200;

    let mut pe = vec![0u8; 0x200];
    pe[0] = b'M';
    pe[1] = b'Z';
    pe[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    pe[0x80..0x84].copy_from_slice(b"PE\0\0");
    let coff = 0x84;
    pe[coff..coff + 2].copy_from_slice(&0x14cu16.to_le_bytes());
    pe[coff + 2..coff + 4].copy_from_slice(&1u16.to_le_bytes());
    pe[coff + 16..coff + 18].copy_from_slice(&224u16.to_le_bytes());
    pe[coff + 18..coff + 20].copy_from_slice(&0x2102u16.to_le_bytes());
    let opt = coff + 20;
    pe[opt..opt + 2].copy_from_slice(&0x10bu16.to_le_bytes());
    pe[opt + 28..opt + 32].copy_from_slice(&0x40_0000u32.to_le_bytes());
    pe[opt + 32..opt + 36].copy_from_slice(&0x2000u32.to_le_bytes());
    pe[opt + 36..opt + 40].copy_from_slice(&0x200u32.to_le_bytes());
    pe[opt + 56..opt + 60].copy_from_slice(&(TEXT_RVA + 0x2000 * (raw_size as u32).div_ceil(0x2000)).to_le_bytes());
    pe[opt + 60..opt + 64].copy_from_slice(&0x200u32.to_le_bytes());
    pe[opt + 68..opt + 70].copy_from_slice(&3u16.to_le_bytes());
    pe[opt + 92..opt + 96].copy_from_slice(&16u32.to_le_bytes());
    let dd14 = opt + 96 + 14 * 8;
    pe[dd14..dd14 + 4].copy_from_slice(&TEXT_RVA.to_le_bytes());
    pe[dd14 + 4..dd14 + 8].copy_from_slice(&72u32.to_le_bytes());
    let sec = opt + 224;
    pe[sec..sec + 5].copy_from_slice(b".text");
    pe[sec + 8..sec + 12].copy_from_slice(&(text.len() as u32).to_le_bytes());
    pe[sec + 12..sec + 16].copy_from_slice(&TEXT_RVA.to_le_bytes());
    pe[sec + 16..sec + 20].copy_from_slice(&(raw_size as u32).to_le_bytes());
    pe[sec + 20..sec + 24].copy_from_slice(&0x200u32.to_le_bytes());
    pe[sec + 36..sec + 40].copy_from_slice(&0x6000_0020u32.to_le_bytes());
    text.resize(raw_size, 0);
    pe.extend(text);
    pe
}

/// Non-generic events, non-generic `MemberRef` calls, malformed unbound fixed placeholders on
/// non-generic `TypeRef` parents, and invalid/over-deep `TypeSpec` parents preserve existing output.
#[test]
fn non_generic_events_and_invalid_or_unbound_parents_remain_unchanged() {
    let bytes = build_generic_context_assembly();
    let asm = dotnet::open(&bytes).unwrap();
    let m = dotnet::members(&asm, &MemberOptions::default());

    let plain = m.types.iter().find(|ty| ty.full_name == "Ns.Plain").unwrap();
    assert_eq!(plain.events.len(), 1);
    assert_eq!(plain.events[0].name, "Ready");
    assert_eq!(plain.events[0].ty, "System.EventHandler");

    let run = plain.methods.iter().find(|x| x.name == "Run").unwrap();
    assert_eq!(run.body.as_ref().unwrap().calls[0].target, "Ext.PlainHelper::Ping(int)");

    let bus = m.types.iter().find(|ty| ty.full_name == "Ns.Bus`2").unwrap();
    let dispatch = bus.methods.iter().find(|x| x.name == "Dispatch").unwrap();
    let calls: Vec<&str> = dispatch.body.as_ref().unwrap().calls.iter().map(|c| c.target.as_str()).collect();
    assert_eq!(calls[4], "Ext.PlainHelper::Raw(!0, !!0)");
    assert_eq!(calls[10], "?::BadSz(!0, !!0)");
    assert_eq!(calls[11], "?::DeepAdd(!0)");
}

/// Events declared on generic and nested generic types resolve `!n` against the owning `TypeDef`
/// generic parameters rather than emitting raw `!0` / `!1` placeholders.
#[test]
fn generic_type_events_resolve_enclosing_type_parameters() {
    let bytes = build_generic_context_assembly();
    let asm = dotnet::open(&bytes).unwrap();
    let m = dotnet::members(&asm, &MemberOptions { skip_bodies: true, ..Default::default() });

    let bus = m.types.iter().find(|ty| ty.full_name == "Ns.Bus`2").unwrap();
    assert_eq!(bus.events.len(), 1);
    assert_eq!(bus.events[0].name, "Fired");
    assert_eq!(bus.events[0].ty, "System.Action<TKey, TVal>");

    let nested = m.types.iter().find(|ty| ty.full_name == "Ns.Bus`2/Nested").unwrap();
    assert_eq!(nested.events.len(), 1);
    assert_eq!(nested.events[0].name, "NestedFired");
    assert_eq!(nested.events[0].ty, "System.Action<TVal>");

    let src = r#"
        namespace Ns {
            public class Bus<TKey, TVal> {
                public event System.Action<TKey, TVal> Fired;
                public TMsg Dispatch<TMsg>(TVal val, TMsg msg) => msg;
                public class Nested {
                    public event System.Action<TVal> NestedFired;
                }
            }
        }
    "#;
    let rec = csharp::reconcile(&m, &csharp::scan(src));
    let fired = rec.matched.iter().find(|x| x.kind == "event" && x.name == "Fired").unwrap();
    assert_eq!(fired.metadata.as_deref(), Some("event System.Action<TKey, TVal> Fired"));
}

/// `MemberRef` method calls and field accesses on `TypeSpec` (including long `modopt` chains) and
/// `TypeDef` parents bind `!n` to the target type's parameters rather than caller positions.
#[test]
fn generic_method_body_calls_and_field_accesses_resolve_context_parameters() {
    let bytes = build_generic_context_assembly();
    let asm = dotnet::open(&bytes).unwrap();
    let m = dotnet::members(&asm, &MemberOptions::default());

    let bus = m.types.iter().find(|ty| ty.full_name == "Ns.Bus`2").unwrap();
    let dispatch = bus.methods.iter().find(|x| x.name == "Dispatch").unwrap();
    let body = dispatch.body.as_ref().unwrap();

    assert_eq!(body.calls[0].target, "System.Collections.Generic.Dictionary<TVal, TMsg>::Add(TVal, TMsg)");
    assert_eq!(body.calls[6].target, "System.Collections.Generic.List<TVal>::Add(TVal)");
    assert_eq!(body.calls[7].target, "Ns.LocalGen`1::Take(TItem)");
    assert_eq!(body.field_accesses.len(), 1);
    assert_eq!(body.field_accesses[0].target, "Ns.Holder<TVal>::Item");

    let plain = m.types.iter().find(|ty| ty.full_name == "Ns.Plain").unwrap();
    let run = plain.methods.iter().find(|x| x.name == "Run").unwrap();
    assert_eq!(run.body.as_ref().unwrap().calls[1].target, "System.Collections.Generic.List<int>::Add(int)");
}

/// `MethodSpec` instantiations, 1D/2D generic array pseudo-methods, non-generic `MemberRef` guards,
/// and `vararg` post-sentinel arguments resolve against their respective ECMA-335 contexts.
#[test]
fn generic_method_specs_arrays_and_vararg_sentinels_resolve_accurately() {
    let bytes = build_generic_context_assembly();
    let asm = dotnet::open(&bytes).unwrap();
    let m = dotnet::members(&asm, &MemberOptions::default());

    let bus = m.types.iter().find(|ty| ty.full_name == "Ns.Bus`2").unwrap();
    let dispatch = bus.methods.iter().find(|x| x.name == "Dispatch").unwrap();
    let calls: Vec<&str> = dispatch.body.as_ref().unwrap().calls.iter().map(|c| c.target.as_str()).collect();

    assert_eq!(calls[1], "Ns.Holder<TVal>::Convert(TVal, TMsg)<TMsg>");
    assert_eq!(calls[2], "Ns.Util::Identity(U)<TMsg>");
    assert_eq!(calls[3], "TMsg[,]::Set(int, int, TMsg)");
    assert_eq!(calls[5], "TVal[]::Set(int, TVal)");
    assert_eq!(calls[8], "Ext.PlainHelper::LogVararg(!0, TKey, TVal, TMsg)");
    assert_eq!(calls[9], "Ns.Holder<TVal>::NonGen(!!0)<TMsg>");
}

/// Decoded IL instruction listings (`include_il: true`) resolve `TypeSpec`, `MemberRef`, and
/// `MethodSpec` token operands against the enclosing method's generic context.
#[test]
fn il_instruction_listing_resolves_typespec_and_token_operands_in_generic_context() {
    let bytes = build_generic_context_assembly();
    let asm = dotnet::open(&bytes).unwrap();
    let m = dotnet::members(&asm, &MemberOptions { include_il: true, ..Default::default() });

    let bus = m.types.iter().find(|ty| ty.full_name == "Ns.Bus`2").unwrap();
    let dispatch = bus.methods.iter().find(|x| x.name == "Dispatch").unwrap();
    let il = dispatch.body.as_ref().unwrap().il.as_ref().unwrap();

    assert_eq!(il[0], "IL_0000: box TVal");
    assert_eq!(il[1], "IL_0005: newarr TMsg");
    assert_eq!(il[2], "IL_000a: ldtoken TMsg");
    assert_eq!(il[3], "IL_000f: callvirt System.Collections.Generic.Dictionary<TVal, TMsg>::Add(TVal, TMsg)");
    assert_eq!(il[4], "IL_0014: stfld Ns.Holder<TVal>::Item");
    assert_eq!(il[5], "IL_0019: callvirt Ns.Holder<TVal>::Convert(TVal, TMsg)<TMsg>");
    assert_eq!(il[6], "IL_001e: call Ns.Util::Identity(U)<TMsg>");
    assert_eq!(il[7], "IL_0023: call TMsg[,]::Set(int, int, TMsg)");
}
