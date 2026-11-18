//! Regression tests for C# type normalization and metadata signature reconciliation.

use craft_anything_formats::dotnet::csharp::{norm_type, reconcile, scan};
use craft_anything_formats::dotnet::{Members, MethodInfo, Param, TypeInfo};

/// Construct a minimal metadata method entry for reconciliation checks.
fn meta_method(name: &str, params: &[&str], ret: &str) -> MethodInfo {
    MethodInfo {
        token: String::new(),
        name: name.into(),
        key: String::new(),
        signature: format!("{ret} {name}({})", params.join(", ")),
        return_type: ret.into(),
        params: params.iter().map(|t| Param { name: String::new(), ty: (*t).into() }).collect(),
        generic_params: vec![],
        visibility: "public",
        modifiers: vec![],
        code_type: "il",
        internal_call: false,
        pinvoke: false,
        rva: 0,
        body: None,
    }
}

/// Construct a minimal metadata type entry holding `methods`.
fn meta_type(full_name: &str, methods: Vec<MethodInfo>) -> TypeInfo {
    let (namespace, name) = full_name.rsplit_once('.').unwrap_or(("", full_name));
    TypeInfo {
        token: "0x02000002".into(),
        full_name: full_name.into(),
        namespace: namespace.into(),
        name: name.into(),
        kind: "class",
        visibility: "public",
        modifiers: vec![],
        extends: Some("object".into()),
        enclosing_type: None,
        generic_params: vec![],
        interfaces: vec![],
        fields: vec![],
        methods,
        properties: vec![],
        events: vec![],
    }
}

/// Baseline control: ordinary built-in aliases, `ref`/`out` parameters, and generic types reconcile cleanly.
#[test]
fn ordinary_builtin_and_byref_parameters_continue_to_reconcile() {
    assert_eq!(norm_type("System.Collections.Generic.List<System.Int32>"), "List<int>");
    assert_eq!(norm_type("ref System.String"), "ref string");

    let src = r#"
        namespace Demo {
            public class Service {
                public void Run(ref int count, out string label) { label = ""; }
            }
        }
    "#;
    let scanned = scan(src);
    let members = Members {
        types: vec![meta_type("Demo.Service", vec![meta_method(".ctor", &[], "void"), meta_method("Run", &["ref int", "ref string"], "void")])],
        total_types: 1,
        total_methods: 2,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&members, &scanned);
    assert!(rec.missing.is_empty(), "{:?}", rec.missing);
    assert!(rec.extra.is_empty(), "{:?}", rec.extra);
    assert!(rec.signature_mismatch.is_empty(), "{:?}", rec.signature_mismatch);
}

/// Identifiers containing `ref` (`Preference`, `Xref`, `App.Prefs`, `@ref`) must not gain spurious spaces.
#[test]
fn identifiers_containing_ref_substring_or_verbatim_ref_keep_exact_names() {
    assert_eq!(norm_type("Preference"), "Preference");
    assert_eq!(norm_type("Xref"), "Xref");
    assert_eq!(norm_type("App.Prefs.ref"), "ref");
    assert_eq!(norm_type("ref App.Prefs.ref"), "ref ref");
    assert_eq!(norm_type("List<App.Prefs.ref>"), "List<ref>");

    let src = r#"
        namespace App.Prefs {
            public class Outer { public class Inner { } }
            public class @ref { }
            public class Box {
                public void UseVal(@ref item) { }
                public void UseRef(ref @ref item) { }
                public static implicit operator Outer.Inner(Box b) => null;
            }
        }
    "#;
    let scanned = scan(src);
    let members = Members {
        types: vec![
            meta_type("App.Prefs.Outer", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.Prefs.Outer/Inner", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.Prefs.ref", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Prefs.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("UseVal", &["App.Prefs.ref"], "void"),
                    meta_method("UseRef", &["ref App.Prefs.ref"], "void"),
                    meta_method("op_Implicit", &["App.Prefs.Box"], "App.Prefs.Outer/Inner"),
                ],
            ),
        ],
        total_types: 4,
        total_methods: 7,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&members, &scanned);
    assert!(rec.missing.is_empty(), "missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "extra: {:?}", rec.extra);
    assert!(rec.signature_mismatch.is_empty(), "mismatch: {:?}", rec.signature_mismatch);
}

/// Metadata `/` nested-type separators and source `global::` qualifiers in method parameters reconcile cleanly.
#[test]
fn nested_type_slash_separators_and_global_qualifiers_reconcile_in_parameters() {
    assert_eq!(norm_type("App.Outer/Inner"), "Inner");
    assert_eq!(norm_type("global::App.Outer.Inner"), "Inner");
    assert_eq!(norm_type("ref global::System.Int32"), "ref int");

    let src = r#"
        namespace App {
            public class Outer { public class Inner { } }
            public class Consumer {
                public void TakeShort(Outer.Inner item) { }
                public void TakeGlobal(global::App.Outer item, ref global::System.Int32 count) { }
            }
        }
    "#;
    let scanned = scan(src);
    let members = Members {
        types: vec![
            meta_type("App.Outer", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.Outer/Inner", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Consumer",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("TakeShort", &["App.Outer/Inner"], "void"),
                    meta_method("TakeGlobal", &["App.Outer", "ref int"], "void"),
                ],
            ),
        ],
        total_types: 3,
        total_methods: 5,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&members, &scanned);
    assert!(rec.missing.is_empty(), "missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "extra: {:?}", rec.extra);
    assert!(rec.signature_mismatch.is_empty(), "mismatch: {:?}", rec.signature_mismatch);
}

/// User types sharing a CLR built-in simple name (`App.String`, `Other.String`) stay distinct from `System.String` in conversion targets.
#[test]
fn non_system_types_named_like_clr_builtins_do_not_collapse_to_keywords() {
    let src = "namespace App {\n\
        public class String { }\n\
        public class Box {\n\
            public static implicit operator string(Box b) => \"\";\n\
            public static implicit operator String(Box b) => null;\n\
            public static implicit operator Other.String(Box b) => null;\n\
        }\n\
        public class Box2 {\n\
            public static implicit operator string(Box2 b) => \"\";\n\
            public static implicit operator App.String(Box2 b) => null;\n\
        }\n\
    }\n\
    namespace Other { public class String { } }\n";
    let scanned = scan(src);
    let members = Members {
        types: vec![
            meta_type("App.String", vec![meta_method(".ctor", &[], "void")]),
            meta_type("Other.String", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "App.String"),
                    meta_method("op_Implicit", &["App.Box"], "string"),
                    meta_method("op_Implicit", &["App.Box"], "Other.String"),
                ],
            ),
            meta_type(
                "App.Box2",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box2"], "App.String"),
                    meta_method("op_Implicit", &["App.Box2"], "string"),
                ],
            ),
        ],
        total_types: 4,
        total_methods: 9,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&members, &scanned);
    assert!(rec.missing.is_empty(), "missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "extra: {:?}", rec.extra);
    assert!(rec.signature_mismatch.is_empty(), "mismatch: {:?}", rec.signature_mismatch);
    let matched_lines: Vec<(&str, Option<u32>)> = rec.matched.iter().map(|m| (m.type_name.as_str(), m.line)).collect();
    assert_eq!(matched_lines, vec![("App.Box", Some(5)), ("App.Box", Some(4)), ("App.Box", Some(6)), ("App.Box2", Some(10)), ("App.Box2", Some(9))]);
}

/// Zero-based multidimensional CLI array shapes (`int[0...,0...]`) reconcile with C# `int[,]` while rank-1 `int[0...]` stays distinct from `int[]`.
#[test]
fn multidimensional_zero_lower_bound_arrays_reconcile_without_conflating_rank_one_arrays() {
    assert_eq!(norm_type("int[0...,0...]"), "int[,]");
    assert_eq!(norm_type("System.String[0...,0...,0...]"), "string[,,]");
    assert_eq!(norm_type("int[0...]"), "int[0...]");
    assert_eq!(norm_type("int[]"), "int[]");

    let src = r#"
        namespace App {
            public class Matrix {
                public static implicit operator int[](Matrix m) => null;
                public static implicit operator int[,](Matrix m) => null;
            }
        }
    "#;
    let scanned = scan(src);
    let members = Members {
        types: vec![meta_type(
            "App.Matrix",
            vec![
                meta_method(".ctor", &[], "void"),
                meta_method("op_Implicit", &["App.Matrix"], "int[0...,0...]"),
                meta_method("op_Implicit", &["App.Matrix"], "int[]"),
            ],
        )],
        total_types: 1,
        total_methods: 3,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&members, &scanned);
    assert!(rec.missing.is_empty(), "missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "extra: {:?}", rec.extra);
    let lines: Vec<Option<u32>> = rec.matched.iter().map(|m| m.line).collect();
    assert_eq!(lines, vec![Some(5), Some(4)]);
}

/// Qualified generic conversion targets (`System.Collections.Generic.List<String>`) resolve unshadowed CLR built-in arguments while preserving `App.String`.
#[test]
fn qualified_generic_conversion_targets_resolve_unshadowed_builtin_arguments() {
    let unshadowed_src = r#"
        using System;
        namespace App {
            public class Box {
                public static implicit operator System.Collections.Generic.List<String>(Box b) => null;
                public static implicit operator System.Collections.Generic.List<Other.String>(Box b) => null;
            }
        }
        namespace Other { public class String { } }
    "#;
    let unshadowed_scan = scan(unshadowed_src);
    let unshadowed_members = Members {
        types: vec![
            meta_type("Other.String", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<Other.String>"),
                    meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<string>"),
                ],
            ),
        ],
        total_types: 2,
        total_methods: 4,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&unshadowed_members, &unshadowed_scan);
    assert!(rec.missing.is_empty(), "unshadowed missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "unshadowed extra: {:?}", rec.extra);
    assert_eq!(rec.matched.iter().map(|m| m.line).collect::<Vec<_>>(), vec![Some(6), Some(5)]);

    let shadowed_src = r#"
        namespace App {
            public class String { }
            public class Box {
                public static implicit operator System.Collections.Generic.List<String>(Box b) => null;
                public static implicit operator System.Collections.Generic.List<string>(Box b) => null;
            }
        }
    "#;
    let shadowed_scan = scan(shadowed_src);
    let shadowed_members = Members {
        types: vec![
            meta_type("App.String", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<string>"),
                    meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<App.String>"),
                ],
            ),
        ],
        total_types: 2,
        total_methods: 4,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let shadowed_rec = reconcile(&shadowed_members, &shadowed_scan);
    assert!(shadowed_rec.missing.is_empty(), "shadowed missing: {:?}", shadowed_rec.missing);
    assert!(shadowed_rec.extra.is_empty(), "shadowed extra: {:?}", shadowed_rec.extra);
    assert_eq!(shadowed_rec.matched.iter().map(|m| m.line).collect::<Vec<_>>(), vec![Some(6), Some(5)]);
}

/// An unrelated `Other.String` conversion must not block `String` from matching `System.String` when `App.String` does not exist.
#[test]
fn unrelated_qualified_type_does_not_veto_unshadowed_builtin_alias_conversion() {
    let src = r#"
        using System;
        namespace App {
            public class Box {
                public static implicit operator String(Box b) => "";
                public static implicit operator Other.String(Box b) => null;
            }
        }
        namespace Other { public class String { } }
    "#;
    let scanned = scan(src);
    let members = Members {
        types: vec![
            meta_type("Other.String", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "Other.String"),
                    meta_method("op_Implicit", &["App.Box"], "string"),
                ],
            ),
        ],
        total_types: 2,
        total_methods: 4,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&members, &scanned);
    assert!(rec.missing.is_empty(), "missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "extra: {:?}", rec.extra);
    assert_eq!(rec.matched.iter().map(|m| m.line).collect::<Vec<_>>(), vec![Some(6), Some(5)]);

    // Without `using System;`, `using Other;` lets `String` target `Other.String` alongside lowercase `string`.
    let imported_other_src = r#"
        using Other;
        namespace App {
            public class Box {
                public static implicit operator String(Box b) => null;
                public static implicit operator string(Box b) => "";
            }
        }
        namespace Other { public class String { } }
    "#;
    let imported_other_scan = scan(imported_other_src);
    let imported_other_rec = reconcile(&members, &imported_other_scan);
    assert!(imported_other_rec.missing.is_empty(), "imported_other missing: {:?}", imported_other_rec.missing);
    assert!(imported_other_rec.extra.is_empty(), "imported_other extra: {:?}", imported_other_rec.extra);
    assert_eq!(imported_other_rec.matched.iter().map(|m| m.line).collect::<Vec<_>>(), vec![Some(5), Some(6)]);
}

/// Generic `App.String<T>` (arity 1) must not shadow zero-arity `String` under `using System;`.
#[test]
fn generic_arity_mismatch_does_not_shadow_zero_arity_builtin_alias() {
    let src = r#"
        using System;
        namespace App {
            public class String<T> { }
            public class Box {
                public static implicit operator String(Box b) => "";
                public static implicit operator String<int>(Box b) => null;
            }
        }
    "#;
    let scanned = scan(src);
    let members = Members {
        types: vec![
            meta_type("App.String`1", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "string"),
                    meta_method("op_Implicit", &["App.Box"], "App.String<int>"),
                ],
            ),
        ],
        total_types: 2,
        total_methods: 4,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&members, &scanned);
    assert!(rec.missing.is_empty(), "missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "extra: {:?}", rec.extra);
    assert_eq!(rec.matched.iter().map(|m| m.line).collect::<Vec<_>>(), vec![Some(6), Some(7)]);
}

/// Predefined keyword `string` stays `System.String` even when `App.@string` is declared in scope.
#[test]
fn predefined_keyword_string_is_not_shadowed_by_verbatim_type_declaration() {
    let src = r#"
        namespace App {
            public class @string { }
            public class Box {
                public static implicit operator string(Box b) => "";
                public static implicit operator @string(Box b) => null;
            }
        }
    "#;
    let scanned = scan(src);
    let members = Members {
        types: vec![
            meta_type("App.string", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "App.string"),
                    meta_method("op_Implicit", &["App.Box"], "string"),
                ],
            ),
        ],
        total_types: 2,
        total_methods: 4,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&members, &scanned);
    assert!(rec.missing.is_empty(), "missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "extra: {:?}", rec.extra);
    assert_eq!(rec.matched.iter().map(|m| m.line).collect::<Vec<_>>(), vec![Some(6), Some(5)]);
}

/// `global::System.Collections.Generic.List<String>` still resolves unshadowed `String` inside generic arguments.
#[test]
fn global_qualified_outer_generic_still_resolves_inner_type_arguments() {
    let src = r#"
        using System;
        namespace App {
            public class Box {
                public static implicit operator global::System.Collections.Generic.List<String>(Box b) => null;
            }
        }
    "#;
    let scanned = scan(src);
    let members = Members {
        types: vec![meta_type(
            "App.Box",
            vec![meta_method(".ctor", &[], "void"), meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<string>")],
        )],
        total_types: 1,
        total_methods: 2,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&members, &scanned);
    assert!(rec.missing.is_empty(), "missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "extra: {:?}", rec.extra);
}

/// Enclosing lookup stops at `App.String` and does not re-prefix it to match `App.App.String`.
#[test]
fn resolved_enclosing_type_is_not_prefixed_a_second_time() {
    let src = r#"
        namespace App {
            public class String { }
            namespace App {
                public class String { }
            }
            public class Box {
                public static implicit operator String(Box b) => null;
                public static implicit operator App.String(Box b) => null;
            }
        }
    "#;
    let scanned = scan(src);
    let members = Members {
        types: vec![
            meta_type("App.String", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.App.String", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "App.App.String"),
                    meta_method("op_Implicit", &["App.Box"], "App.String"),
                ],
            ),
        ],
        total_types: 3,
        total_methods: 5,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&members, &scanned);
    assert!(rec.missing.is_empty(), "missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "extra: {:?}", rec.extra);
    assert_eq!(rec.matched.iter().map(|m| m.line).collect::<Vec<_>>(), vec![Some(9), Some(8)]);
}

/// Terminal fallback for an imported generic (`List<String>`) preserves resolved argument identities (`App.String` vs `string`).
#[test]
fn terminal_fallback_preserves_resolved_generic_argument_identities() {
    let shadowed_src = r#"
        using System.Collections.Generic;
        namespace App {
            public class String { }
            public class Box {
                public static implicit operator List<String>(Box b) => null;
            }
        }
    "#;
    let shadowed_scan = scan(shadowed_src);
    let wrong_members = Members {
        types: vec![
            meta_type("App.String", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.Box", vec![meta_method(".ctor", &[], "void"), meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<string>")]),
        ],
        total_types: 2,
        total_methods: 3,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let wrong_rec = reconcile(&wrong_members, &shadowed_scan);
    assert_eq!(wrong_rec.missing.len(), 1, "expected mismatch when App.String shadows string: {:?}", wrong_rec);
    assert_eq!(wrong_rec.extra.len(), 1, "expected extra when App.String shadows string: {:?}", wrong_rec);

    let unshadowed_src = r#"
        using System;
        using System.Collections.Generic;
        namespace App {
            public class Box {
                public static implicit operator List<String>(Box b) => null;
            }
        }
    "#;
    let unshadowed_scan = scan(unshadowed_src);
    let unshadowed_members = Members {
        types: vec![meta_type(
            "App.Box",
            vec![meta_method(".ctor", &[], "void"), meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<string>")],
        )],
        total_types: 1,
        total_methods: 2,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let unshadowed_rec = reconcile(&unshadowed_members, &unshadowed_scan);
    assert!(unshadowed_rec.missing.is_empty(), "unshadowed List<String> missing: {:?}", unshadowed_rec.missing);
    assert!(unshadowed_rec.extra.is_empty(), "unshadowed List<String> extra: {:?}", unshadowed_rec.extra);
}

/// Contextual type keywords (`nint`, `nuint`, `dynamic`) resolve to declared types in scope before
/// falling back to built-in meanings, while arity-mismatched `App.nint<T>` does not shadow `nint`.
#[test]
fn contextual_keywords_resolve_declared_types_before_builtin_fallback() {
    let shadowed_src = r#"
        namespace App {
            public class nint { }
            public class nuint { }
            public class dynamic { }
            public class Box {
                public static implicit operator nint(Box b) => null;
                public static implicit operator nuint(Box b) => null;
                public static implicit operator dynamic(Box b) => null;
                public static implicit operator global::System.IntPtr(Box b) => 0;
            }
        }
    "#;
    let shadowed_scan = scan(shadowed_src);
    let shadowed_members = Members {
        types: vec![
            meta_type("App.nint", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.nuint", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.dynamic", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "nint"),
                    meta_method("op_Implicit", &["App.Box"], "App.dynamic"),
                    meta_method("op_Implicit", &["App.Box"], "App.nuint"),
                    meta_method("op_Implicit", &["App.Box"], "App.nint"),
                ],
            ),
        ],
        total_types: 4,
        total_methods: 8,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let rec = reconcile(&shadowed_members, &shadowed_scan);
    assert!(rec.missing.is_empty(), "shadowed contextual missing: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "shadowed contextual extra: {:?}", rec.extra);
    assert_eq!(rec.matched.iter().map(|m| m.line).collect::<Vec<_>>(), vec![Some(10), Some(9), Some(8), Some(7)]);

    // Declared `App.nint`/`App.nuint`/`App.dynamic` shadow bare contextual keywords against built-in
    // metadata (`nint`/`nuint`/`object`), while non-`global::` `System.IntPtr`/`System.UIntPtr` still
    // resolve to built-in `nint`/`nuint` even when declared BEFORE `nint`/`nuint` in source.
    let mismatch_src = r#"
        namespace App {
            public class nint { }
            public class nuint { }
            public class dynamic { }
            public class Box {
                public static implicit operator System.IntPtr(Box b) => 0;
                public static implicit operator System.UIntPtr(Box b) => 0;
                public static implicit operator nint(Box b) => null;
                public static implicit operator nuint(Box b) => null;
                public static implicit operator dynamic(Box b) => null;
            }
        }
    "#;
    let mismatch_members = Members {
        types: vec![
            meta_type("App.nint", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.nuint", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.dynamic", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "App.nint"),
                    meta_method("op_Implicit", &["App.Box"], "App.nuint"),
                    meta_method("op_Implicit", &["App.Box"], "nint"),
                    meta_method("op_Implicit", &["App.Box"], "nuint"),
                    meta_method("op_Implicit", &["App.Box"], "App.dynamic"),
                ],
            ),
        ],
        total_types: 4,
        total_methods: 9,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let mismatch_rec = reconcile(&mismatch_members, &scan(mismatch_src));
    assert!(mismatch_rec.missing.is_empty(), "expected shadowed App.nint/nuint/dynamic + explicit System.IntPtr/UIntPtr missing: {:?}", mismatch_rec.missing);
    assert!(mismatch_rec.extra.is_empty(), "expected shadowed App.nint/nuint/dynamic + explicit System.IntPtr/UIntPtr extra: {:?}", mismatch_rec.extra);
    assert_eq!(mismatch_rec.matched.iter().map(|m| m.line).collect::<Vec<_>>(), vec![Some(9), Some(10), Some(7), Some(8), Some(11)]);

    // Declared `App.nint`/`App.nuint`/`App.dynamic` alone must not match built-in `nint`/`nuint`/`object`.
    let blocked_src = r#"
        namespace App {
            public class nint { }
            public class nuint { }
            public class dynamic { }
            public class Box {
                public static implicit operator nint(Box b) => null;
                public static implicit operator nuint(Box b) => null;
                public static implicit operator dynamic(Box b) => null;
            }
        }
    "#;
    let blocked_members = Members {
        types: vec![
            meta_type("App.nint", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.nuint", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.dynamic", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "nint"),
                    meta_method("op_Implicit", &["App.Box"], "nuint"),
                    meta_method("op_Implicit", &["App.Box"], "object"),
                ],
            ),
        ],
        total_types: 4,
        total_methods: 7,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let blocked_rec = reconcile(&blocked_members, &scan(blocked_src));
    assert_eq!(blocked_rec.missing.len(), 3, "expected 3 missing when App.nint/nuint/dynamic shadow built-ins: {:?}", blocked_rec);
    assert_eq!(blocked_rec.extra.len(), 3, "expected 3 extra when App.nint/nuint/dynamic shadow built-ins: {:?}", blocked_rec);

    // Without zero-arity declarations in scope (only `App.nint<T>`), bare contextual `nint` falls
    // back to built-in `nint`, and imported `Other.nint` resolves via terminal fallback.
    let builtin_src = r#"
        namespace App {
            public class nint<T> { }
            public class Box {
                public static implicit operator nint(Box b) => 0;
            }
        }
    "#;
    let builtin_members = Members {
        types: vec![
            meta_type("App.nint`1", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.Box", vec![meta_method(".ctor", &[], "void"), meta_method("op_Implicit", &["App.Box"], "nint")]),
        ],
        total_types: 2,
        total_methods: 3,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let builtin_rec = reconcile(&builtin_members, &scan(builtin_src));
    assert!(builtin_rec.missing.is_empty(), "unshadowed bare nint missing: {:?}", builtin_rec.missing);
    assert!(builtin_rec.extra.is_empty(), "unshadowed bare nint extra: {:?}", builtin_rec.extra);

    let fallback_src = r#"
        using Other;
        namespace Other { public class nint { } }
        namespace App {
            public class nint<T> { }
            public class Box {
                public static implicit operator global::System.IntPtr(Box b) => 0;
                public static implicit operator nuint(Box b) => 0;
                public static implicit operator dynamic(Box b) => null;
            }
            public class Holder {
                public static implicit operator nint(Holder h) => null;
            }
        }
    "#;
    let fallback_members = Members {
        types: vec![
            meta_type("Other.nint", vec![meta_method(".ctor", &[], "void")]),
            meta_type("App.nint`1", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "nint"),
                    meta_method("op_Implicit", &["App.Box"], "nuint"),
                    meta_method("op_Implicit", &["App.Box"], "object"),
                ],
            ),
            meta_type("App.Holder", vec![meta_method(".ctor", &[], "void"), meta_method("op_Implicit", &["App.Holder"], "Other.nint")]),
        ],
        total_types: 4,
        total_methods: 8,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let fallback_rec = reconcile(&fallback_members, &scan(fallback_src));
    assert!(fallback_rec.missing.is_empty(), "fallback contextual missing: {:?}", fallback_rec.missing);
    assert!(fallback_rec.extra.is_empty(), "fallback contextual extra: {:?}", fallback_rec.extra);
}

/// Terminal fallback for imported generic types (`List<String>`) compares scope-resolved arguments before
/// alias-expanded arguments so `Other.String` matches without losing `System.String` prioritization.
#[test]
fn terminal_fallback_prefers_scope_resolved_arguments_over_alias_expansion() {
    let imported_src = r#"
        using Other;
        using System.Collections.Generic;
        namespace Other { public class String { } }
        namespace App {
            public class Box {
                public static implicit operator List<String>(Box b) => null;
            }
        }
    "#;
    let imported_members = Members {
        types: vec![
            meta_type("Other.String", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![meta_method(".ctor", &[], "void"), meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<Other.String>")],
            ),
        ],
        total_types: 2,
        total_methods: 3,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let imported_rec = reconcile(&imported_members, &scan(imported_src));
    assert!(imported_rec.missing.is_empty(), "imported List<Other.String> missing: {:?}", imported_rec.missing);
    assert!(imported_rec.extra.is_empty(), "imported List<Other.String> extra: {:?}", imported_rec.extra);

    // When both `List<String>` (`Other.String`) and `List<string>` (`System.String`) coexist on one type,
    // scope-resolved `List<string>` must take priority for metadata `List<string>` even when `List<String>` appears first.
    let mixed_src = r#"
        using Other;
        using System.Collections.Generic;
        namespace Other { public class String { } }
        namespace App {
            public class Box {
                public static implicit operator List<String>(Box b) => null;
                public static implicit operator List<string>(Box b) => null;
            }
        }
    "#;
    let mixed_members = Members {
        types: vec![
            meta_type("Other.String", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<string>"),
                    meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<Other.String>"),
                ],
            ),
        ],
        total_types: 2,
        total_methods: 4,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let mixed_rec = reconcile(&mixed_members, &scan(mixed_src));
    assert!(mixed_rec.missing.is_empty(), "mixed List<String>/List<string> missing: {:?}", mixed_rec.missing);
    assert!(mixed_rec.extra.is_empty(), "mixed List<String>/List<string> extra: {:?}", mixed_rec.extra);
    assert_eq!(mixed_rec.matched.iter().map(|m| m.line).collect::<Vec<_>>(), vec![Some(8), Some(7)]);

    // Contextual `nint`/`nuint`/`dynamic` inside generic terminal fallback resolve to imported types
    // (`Other.nint`/`Other.nuint`/`Other.dynamic`), while reserved `string`, explicit
    // `System.IntPtr`/`global::System.IntPtr`/`System.UIntPtr`, and CLR alias-expanded `IntPtr`/`UIntPtr`/`dynamic`
    // must never match `Other.nint`/`Other.nuint`/`Other.object` (while `IntPtr`/`UIntPtr` still match
    // built-in `List<nint>`/`List<nuint>` and `Other.IntPtr`/`Other.UIntPtr`, and `dynamic` matches `object`).
    let generic_kw_src = r#"
        using Other;
        using System.Collections.Generic;
        namespace Other {
            public class nint { }
            public class nuint { }
            public class dynamic { }
            public class IntPtr { }
            public class UIntPtr { }
            public class @object { }
            public class @string { }
        }
        namespace App {
            public class Box {
                public static implicit operator List<nint>(Box b) => null;
                public static implicit operator List<nuint>(Box b) => null;
                public static implicit operator List<dynamic>(Box b) => null;
            }
            public class MixedBox {
                public static implicit operator List<global::System.IntPtr>(MixedBox b) => null;
                public static implicit operator List<nint>(MixedBox b) => null;
            }
            public class AliasBox {
                public static implicit operator List<IntPtr>(AliasBox b) => null;
                public static implicit operator List<UIntPtr>(AliasBox b) => null;
            }
            public class AliasControlBox {
                public static implicit operator List<IntPtr>(AliasControlBox b) => null;
                public static implicit operator List<UIntPtr>(AliasControlBox b) => null;
                public static implicit operator List<dynamic>(AliasControlBox b) => null;
            }
            public class BadBox {
                public static implicit operator List<string>(BadBox b) => null;
                public static implicit operator List<global::System.IntPtr>(BadBox b) => null;
                public static implicit operator List<System.IntPtr>(BadBox b) => null;
                public static implicit operator List<System.UIntPtr>(BadBox b) => null;
                public static implicit operator List<IntPtr>(BadBox b) => null;
                public static implicit operator List<UIntPtr>(BadBox b) => null;
                public static implicit operator List<dynamic>(BadBox b) => null;
            }
        }
    "#;
    let generic_kw_members = Members {
        types: vec![
            meta_type("Other.nint", vec![meta_method(".ctor", &[], "void")]),
            meta_type("Other.nuint", vec![meta_method(".ctor", &[], "void")]),
            meta_type("Other.dynamic", vec![meta_method(".ctor", &[], "void")]),
            meta_type("Other.IntPtr", vec![meta_method(".ctor", &[], "void")]),
            meta_type("Other.UIntPtr", vec![meta_method(".ctor", &[], "void")]),
            meta_type("Other.object", vec![meta_method(".ctor", &[], "void")]),
            meta_type("Other.string", vec![meta_method(".ctor", &[], "void")]),
            meta_type(
                "App.Box",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<Other.nint>"),
                    meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<Other.nuint>"),
                    meta_method("op_Implicit", &["App.Box"], "System.Collections.Generic.List<Other.dynamic>"),
                ],
            ),
            meta_type(
                "App.MixedBox",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.MixedBox"], "System.Collections.Generic.List<Other.nint>"),
                    meta_method("op_Implicit", &["App.MixedBox"], "System.Collections.Generic.List<nint>"),
                ],
            ),
            meta_type(
                "App.AliasBox",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.AliasBox"], "System.Collections.Generic.List<nint>"),
                    meta_method("op_Implicit", &["App.AliasBox"], "System.Collections.Generic.List<nuint>"),
                ],
            ),
            meta_type(
                "App.AliasControlBox",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.AliasControlBox"], "System.Collections.Generic.List<Other.IntPtr>"),
                    meta_method("op_Implicit", &["App.AliasControlBox"], "System.Collections.Generic.List<Other.UIntPtr>"),
                    meta_method("op_Implicit", &["App.AliasControlBox"], "System.Collections.Generic.List<object>"),
                ],
            ),
            meta_type(
                "App.BadBox",
                vec![
                    meta_method(".ctor", &[], "void"),
                    meta_method("op_Implicit", &["App.BadBox"], "System.Collections.Generic.List<Other.string>"),
                    meta_method("op_Implicit", &["App.BadBox"], "System.Collections.Generic.List<Other.nint>"),
                    meta_method("op_Implicit", &["App.BadBox"], "System.Collections.Generic.List<Other.nint>"),
                    meta_method("op_Implicit", &["App.BadBox"], "System.Collections.Generic.List<Other.nuint>"),
                    meta_method("op_Implicit", &["App.BadBox"], "System.Collections.Generic.List<Other.nint>"),
                    meta_method("op_Implicit", &["App.BadBox"], "System.Collections.Generic.List<Other.nuint>"),
                    meta_method("op_Implicit", &["App.BadBox"], "System.Collections.Generic.List<Other.object>"),
                ],
            ),
        ],
        total_types: 12,
        total_methods: 27,
        total_fields: 0,
        truncated: false,
        warnings: vec![],
    };
    let generic_kw_rec = reconcile(&generic_kw_members, &scan(generic_kw_src));
    assert_eq!(generic_kw_rec.missing.len(), 7, "expected only BadBox cases missing: {:?}", generic_kw_rec);
    assert!(generic_kw_rec.missing.iter().all(|m| m.type_name == "App.BadBox"), "{:?}", generic_kw_rec.missing);
    assert_eq!(generic_kw_rec.extra.len(), 7, "expected only BadBox cases extra: {:?}", generic_kw_rec);
    assert!(generic_kw_rec.extra.iter().all(|m| m.type_name == "App.BadBox"), "{:?}", generic_kw_rec.extra);
    let alias_lines: Vec<_> = generic_kw_rec.matched.iter().filter(|m| m.type_name == "App.AliasBox").map(|m| m.line).collect();
    assert_eq!(alias_lines, vec![Some(24), Some(25)], "AliasBox exact List<nint>/List<nuint> matches: {:?}", generic_kw_rec.matched);
    let mixed_lines: Vec<_> = generic_kw_rec.matched.iter().filter(|m| m.type_name == "App.MixedBox").map(|m| m.line).collect();
    assert_eq!(
        mixed_lines,
        vec![Some(21), Some(20)],
        "MixedBox must pair List<nint>->Other.nint and List<global::System.IntPtr>->nint without swapping: {:?}",
        generic_kw_rec.matched
    );
}
