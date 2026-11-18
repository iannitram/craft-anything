use craft_anything_formats::dotnet::{
    csharp,
    meta::{Metadata, t},
    model::{MethodInfo, Param, TypeInfo},
    sig::{GenCtx, Names},
};

/// Append a NUL-terminated UTF-8 entry to the `#Strings` heap and return its 16-bit offset.
fn push_str(heap: &mut Vec<u8>, value: &str) -> u16 {
    let offset = heap.len() as u16;
    heap.extend_from_slice(value.as_bytes());
    heap.push(0);
    offset
}

/// Build a minimal CLI metadata root with nested generic and obfuscated `TypeRef` entries:
/// - row 1: ``Ns.Outer`1``
/// - row 2: ``Ns.Outer`1/Inner`` (enclosing TypeRef row 1)
/// - row 3: ``Ns.Outer`1/Inner2`1`` (enclosing TypeRef row 1)
/// - row 4: ``Ns.Outer`1/Middle`` (enclosing TypeRef row 1)
/// - row 5: ``Ns.Outer`1/Middle/Leaf`2`` (enclosing TypeRef row 4)
/// - row 6: ``Ns.Outer`1/ValInner`` (enclosing TypeRef row 1)
/// - row 7: ``System.Nullable`1``
/// - row 8: `ObfuscatedOuter`
/// - row 9: `ObfuscatedOuter/ObfuscatedInner` (no arity suffixes)
/// - row 10: ``System.Nullable`1/Inner`` (enclosing TypeRef row 7)
/// - row 11: ``Ns.Huge`99999999999999999999999999999`` (overflowing arity)
/// - row 12: ``Ns.Outer`1/Huge`99999999999999999999999999999`` (nested overflowing arity)
/// - row 13: `Ns.PlainOuter`
/// - row 14: ``Ns.PlainOuter/GenericInner`1`` (non-generic outer + generic inner)
/// - row 15: ``System.Nullable`2`` (mismatched Nullable arity)
/// - row 16: ``obf`1/ns.Box`` (obfuscated `/` and backtick in TypeNamespace)
/// - row 17: ``obf`1/ns.Box`1`` (obfuscated `/` and backtick in TypeNamespace + generic TypeName)
/// - row 18: ``A`1/B`` (non-nested TypeName containing `/` and backtick)
/// - row 19: ``M.Outer`1``
/// - row 20: ``M.Outer`1/Inner`` (enclosing TypeRef row 19)
/// - row 21: ``A.Outer`1``
/// - row 22: ``A.Outer`1/Inner1`` (enclosing TypeRef row 21)
/// - row 23: ``B.Outer`1``
/// - row 24: ``B.Outer`1/Inner2`` (enclosing TypeRef row 23)
fn nested_generic_metadata() -> Vec<u8> {
    let mut strings = vec![0];
    let rows_spec: &[(u16, &str, &str)] = &[
        (0, "Outer`1", "Ns"),
        ((1 << 2) | 3, "Inner", ""),
        ((1 << 2) | 3, "Inner2`1", ""),
        ((1 << 2) | 3, "Middle", ""),
        ((4 << 2) | 3, "Leaf`2", ""),
        ((1 << 2) | 3, "ValInner", ""),
        (0, "Nullable`1", "System"),
        (0, "ObfuscatedOuter", ""),
        ((8 << 2) | 3, "ObfuscatedInner", ""),
        ((7 << 2) | 3, "Inner", ""),
        (0, "Huge`99999999999999999999999999999", "Ns"),
        ((1 << 2) | 3, "Huge`99999999999999999999999999999", ""),
        (0, "PlainOuter", "Ns"),
        ((13 << 2) | 3, "GenericInner`1", ""),
        (0, "Nullable`2", "System"),
        (0, "Box", "obf`1/ns"),
        (0, "Box`1", "obf`1/ns"),
        (0, "A`1/B", ""),
        (0, "Outer`1", "M"),
        ((19 << 2) | 3, "Inner", ""),
        (0, "Outer`1", "A"),
        ((21 << 2) | 3, "Inner1", ""),
        (0, "Outer`1", "B"),
        ((23 << 2) | 3, "Inner2", ""),
    ];
    let mut rows = Vec::new();
    for &(scope, name, namespace) in rows_spec {
        let name_idx = push_str(&mut strings, name);
        let ns_idx = if namespace.is_empty() { 0 } else { push_str(&mut strings, namespace) };
        for cell in [scope, name_idx, ns_idx] {
            rows.extend_from_slice(&cell.to_le_bytes());
        }
    }
    let mut tables = vec![0, 0, 0, 0, 2, 0, 0, 1];
    tables.extend_from_slice(&(1u64 << t::TYPE_REF).to_le_bytes());
    tables.extend_from_slice(&0u64.to_le_bytes());
    tables.extend_from_slice(&(rows_spec.len() as u32).to_le_bytes());
    tables.extend_from_slice(&rows);

    let streams = [("#~", tables.as_slice()), ("#Strings", strings.as_slice()), ("#Blob", &[0][..])];
    let mut out = Vec::from(&b"BSJB\x01\0\x01\0\0\0\0\0\x04\0\0\0v4\0\0\0\0\x03\0"[..]);
    let mut offset = out.len() + streams.iter().map(|(name, _)| 8 + (name.len() + 1).div_ceil(4) * 4).sum::<usize>();
    for (name, bytes) in streams {
        out.extend_from_slice(&(offset as u32).to_le_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        out.resize(out.len().div_ceil(4) * 4, 0);
        offset += bytes.len();
    }
    for (_, bytes) in streams {
        out.extend_from_slice(bytes);
    }
    out
}

/// Encode a `TypeRef` row index as a `TypeDefOrRefOrSpecEncoded` unsigned integer (`table = 1`).
const fn typeref_tdor(row: u8) -> u8 {
    (row << 2) | 1
}

/// Non-nested generic instantiations, nullable shorthands, non-generic outer types, and obfuscated
/// `/` or backtick characters inside `TypeNamespace` or non-nested `TypeName` decode unchanged.
#[test]
fn top_level_generic_and_nullable_instantiations_remain_unchanged() {
    let bytes = nested_generic_metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    let g = GenCtx::default();

    assert_eq!(names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(1), 1, 0x08], &g).as_deref(), Some("Ns.Outer<int>"));
    assert_eq!(names.field_sig(&[0x06, 0x15, 0x11, typeref_tdor(7), 1, 0x08], &g).as_deref(), Some("int?"));
    assert_eq!(names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(14), 1, 0x08], &g).as_deref(), Some("Ns.PlainOuter/GenericInner<int>"));
    assert_eq!(names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(16), 1, 0x08], &g).as_deref(), Some("obf`1/ns.Box<int>"));
    assert_eq!(names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(17), 1, 0x08], &g).as_deref(), Some("obf`1/ns.Box<int>"));
    assert_eq!(names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(18), 1, 0x08], &g).as_deref(), Some("A`1/B<int>"));
}

/// Non-generic nested types inside generic outer types (`Outer<int>/Inner`) place type arguments on the outer segment.
#[test]
fn non_generic_nested_type_inside_generic_outer_places_args_on_outer_segment() {
    let bytes = nested_generic_metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    let g = GenCtx::default();

    let inner_sig = [0x06, 0x15, 0x12, typeref_tdor(2), 1, 0x08];
    assert_eq!(names.field_sig(&inner_sig, &g).as_deref(), Some("Ns.Outer<int>/Inner"));

    let nullable_sig = [0x06, 0x15, 0x11, typeref_tdor(7), 1, 0x15, 0x11, typeref_tdor(6), 1, 0x08];
    assert_eq!(names.field_sig(&nullable_sig, &g).as_deref(), Some("Ns.Outer<int>/ValInner?"));

    // A nested type inside ``System.Nullable`1`` is not itself `Nullable<T>` shorthand.
    let nested_under_nullable = [0x06, 0x15, 0x12, typeref_tdor(10), 1, 0x08];
    assert_eq!(names.field_sig(&nested_under_nullable, &g).as_deref(), Some("System.Nullable<int>/Inner"));
}

/// Multi-level nested generics (`Outer<string>/Inner2<int>`, `Outer<int>/Middle/Leaf<string, bool>`) split arguments by per-segment arity.
#[test]
fn multi_level_nested_generic_instantiation_splits_arguments_per_segment_arity() {
    let bytes = nested_generic_metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    let g = GenCtx::default();

    let two_level = [0x06, 0x15, 0x12, typeref_tdor(3), 2, 0x0e, 0x08];
    assert_eq!(names.field_sig(&two_level, &g).as_deref(), Some("Ns.Outer<string>/Inner2<int>"));

    let three_level = [0x06, 0x15, 0x12, typeref_tdor(5), 3, 0x08, 0x0e, 0x02];
    assert_eq!(names.field_sig(&three_level, &g).as_deref(), Some("Ns.Outer<int>/Middle/Leaf<string, bool>"));
}

/// Obfuscated, overflowing, or mismatched arity suffixes strip backtick markers cleanly and retain all type arguments at the end.
#[test]
fn mismatched_or_missing_arity_suffixes_retain_all_type_arguments_without_panicking() {
    let bytes = nested_generic_metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    let g = GenCtx::default();

    let obfuscated = [0x06, 0x15, 0x12, typeref_tdor(9), 2, 0x0e, 0x08];
    assert_eq!(names.field_sig(&obfuscated, &g).as_deref(), Some("ObfuscatedOuter/ObfuscatedInner<string, int>"));

    let mismatched = [0x06, 0x15, 0x12, typeref_tdor(2), 2, 0x0e, 0x08];
    assert_eq!(names.field_sig(&mismatched, &g).as_deref(), Some("Ns.Outer/Inner<string, int>"));

    let wrong_nullable_arity = [0x06, 0x15, 0x11, typeref_tdor(15), 1, 0x08];
    assert_eq!(names.field_sig(&wrong_nullable_arity, &g).as_deref(), Some("System.Nullable<int>"));

    let overflow_arity = [0x06, 0x15, 0x12, typeref_tdor(11), 1, 0x08];
    assert_eq!(names.field_sig(&overflow_arity, &g).as_deref(), Some("Ns.Huge<int>"));

    let nested_overflow_arity = [0x06, 0x15, 0x12, typeref_tdor(12), 1, 0x08];
    assert_eq!(names.field_sig(&nested_overflow_arity, &g).as_deref(), Some("Ns.Outer/Huge<int>"));

    let bare = [0x06, 0x12, typeref_tdor(1)];
    assert_eq!(names.field_sig(&bare, &g).as_deref(), Some("Ns.Outer`1"));
}

/// Decoded nested generic parameter and conversion signatures reconcile cleanly against C# source declarations.
#[test]
fn decoded_nested_generic_signatures_reconcile_with_csharp_declarations() {
    let bytes = nested_generic_metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    let g = GenCtx::default();

    let ty_make1 = names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(2), 1, 0x08], &g).unwrap();
    let ty_make2 = names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(3), 2, 0x0e, 0x08], &g).unwrap();
    let ty_make3 = names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(5), 3, 0x08, 0x0e, 0x02], &g).unwrap();

    let src = r#"
        namespace Ns {
            public class Consumer {
                public Outer<int>.Inner Make1(Outer<int>.Inner x) => x;
                public Outer<string>.Inner2<int> Make2(Outer<string>.Inner2<int> x) => x;
                public Outer<int>.Middle.Leaf<string, bool> Make3(Outer<int>.Middle.Leaf<string, bool> x) => x;
                public static implicit operator Outer<int>.Inner(Consumer c) => null;
            }
        }
    "#;
    let scanned = csharp::scan(src);
    let method = |name: &str, param_ty: &str, ret_ty: &str, mods: &[&'static str]| MethodInfo {
        token: String::new(),
        name: name.into(),
        key: String::new(),
        signature: format!("{ret_ty} {name}({param_ty})"),
        return_type: ret_ty.into(),
        params: vec![Param { name: "x".into(), ty: param_ty.into() }],
        generic_params: vec![],
        visibility: "public",
        modifiers: mods.to_vec(),
        code_type: "il",
        internal_call: false,
        pinvoke: false,
        rva: 0,
        body: None,
    };
    let meta = craft_anything_formats::dotnet::Members {
        types: vec![TypeInfo {
            token: String::new(),
            full_name: "Ns.Consumer".into(),
            namespace: "Ns".into(),
            name: "Consumer".into(),
            kind: "class",
            visibility: "public",
            modifiers: vec![],
            extends: Some("object".into()),
            enclosing_type: None,
            generic_params: vec![],
            interfaces: vec![],
            fields: vec![],
            methods: vec![
                method("Make1", &ty_make1, &ty_make1, &[]),
                method("Make2", &ty_make2, &ty_make2, &[]),
                method("Make3", &ty_make3, &ty_make3, &[]),
                method("op_Implicit", "Ns.Consumer", &ty_make1, &["static"]),
            ],
            properties: vec![],
            events: vec![],
        }],
        total_types: 1,
        total_methods: 4,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = csharp::reconcile(&meta, &scanned);
    assert!(rec.missing.is_empty(), "missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "extra: {:?}", rec.extra);
    assert!(rec.signature_mismatch.is_empty(), "mismatch: {:?}", rec.signature_mismatch);
    assert_eq!(rec.matched.len(), 4);
}

/// Qualified nested generic conversion targets match their exact namespace and do not match unrelated namespaces.
#[test]
fn nested_generic_conversion_targets_preserve_namespace_qualification() {
    let bytes = nested_generic_metadata();
    let md = Metadata::parse(&bytes).unwrap();
    let names = Names::new(&md);
    let g = GenCtx::default();

    let ty_m = names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(20), 1, 0x08], &g).unwrap();
    let ty_a = names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(22), 1, 0x08], &g).unwrap();
    let ty_b = names.field_sig(&[0x06, 0x15, 0x12, typeref_tdor(24), 1, 0x08], &g).unwrap();

    let conv = |param_ty: &str, ret_ty: &str| MethodInfo {
        token: String::new(),
        name: "op_Implicit".into(),
        key: String::new(),
        signature: format!("{ret_ty} op_Implicit({param_ty})"),
        return_type: ret_ty.into(),
        params: vec![Param { name: "c".into(), ty: param_ty.into() }],
        generic_params: vec![],
        visibility: "public",
        modifiers: vec!["static"],
        code_type: "il",
        internal_call: false,
        pinvoke: false,
        rva: 0,
        body: None,
    };

    // Unrelated namespace `M.Outer<int>/Inner` must not match `Outer<int>.Inner` inside `namespace N`.
    let unrelated_meta = craft_anything_formats::dotnet::Members {
        types: vec![TypeInfo {
            token: String::new(),
            full_name: "N.C".into(),
            namespace: "N".into(),
            name: "C".into(),
            kind: "class",
            visibility: "public",
            modifiers: vec![],
            extends: Some("object".into()),
            enclosing_type: None,
            generic_params: vec![],
            interfaces: vec![],
            fields: vec![],
            methods: vec![conv("N.C", &ty_m)],
            properties: vec![],
            events: vec![],
        }],
        total_types: 1,
        total_methods: 1,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let unrelated_scan = csharp::scan("namespace N { class C { public static implicit operator Outer<int>.Inner(C c) => null; } }");
    let unrelated_rec = csharp::reconcile(&unrelated_meta, &unrelated_scan);
    assert_eq!(unrelated_rec.missing.len(), 1, "{:?}", unrelated_rec.missing);
    assert_eq!(unrelated_rec.extra.len(), 1, "{:?}", unrelated_rec.extra);
    assert!(unrelated_rec.matched.is_empty(), "{:?}", unrelated_rec.matched);

    // Distinct fully qualified nested generic targets match their respective lines without conflation.
    let distinct_meta = craft_anything_formats::dotnet::Members {
        types: vec![TypeInfo {
            token: String::new(),
            full_name: "C".into(),
            namespace: String::new(),
            name: "C".into(),
            kind: "class",
            visibility: "public",
            modifiers: vec![],
            extends: Some("object".into()),
            enclosing_type: None,
            generic_params: vec![],
            interfaces: vec![],
            fields: vec![],
            methods: vec![conv("C", &ty_a), conv("C", &ty_b)],
            properties: vec![],
            events: vec![],
        }],
        total_types: 1,
        total_methods: 2,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let distinct_scan = csharp::scan(
        "class C {\n    public static implicit operator B.Outer<int>.Inner2(C c) => null;\n    public static implicit operator A.Outer<int>.Inner1(C c) => null;\n}",
    );
    let distinct_rec = csharp::reconcile(&distinct_meta, &distinct_scan);
    assert!(distinct_rec.missing.is_empty(), "{:?}", distinct_rec.missing);
    assert!(distinct_rec.extra.is_empty(), "{:?}", distinct_rec.extra);
    let lines: Vec<Option<u32>> = distinct_rec.matched.iter().map(|m| m.line).collect();
    assert_eq!(lines, vec![Some(3), Some(2)]);
}
