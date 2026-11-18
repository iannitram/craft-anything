//! Regression tests for C# verbatim `@identifier` scanning and metadata reconciliation.

use craft_anything_formats::dotnet::csharp::{reconcile, scan};
use craft_anything_formats::dotnet::model::PropertyInfo;
use craft_anything_formats::dotnet::{FieldInfo, Members, MethodInfo, Param, TypeInfo};

/// Construct a minimal metadata method entry for reconciliation checks.
fn meta_method(name: &str, params: &[&str], ret: &str) -> MethodInfo {
    MethodInfo {
        token: String::new(),
        name: name.into(),
        key: String::new(),
        signature: name.into(),
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

/// Non-verbatim `namespace`, `class`, `operator`, and `event` keywords continue to scan normally.
#[test]
fn ordinary_keywords_and_operators_continue_to_scan() {
    let scanned = scan(
        r#"
        namespace Demo {
            public class Box {
                public event System.Action Changed;
                public static Box operator +(Box a, Box b) => a;
            }
        }
        "#,
    );
    assert_eq!(scanned.types.len(), 1);
    assert_eq!(scanned.types[0].full_name, "Demo.Box");
    assert!(scanned.members.iter().any(|m| m.kind == "event" && m.name == "Changed"));
    assert!(scanned.members.iter().any(|m| m.kind == "method" && m.name == "op_Addition"));
}

/// Verbatim `@namespace`, `@class`, `@operator`, `@event`, and `@record` members must not trigger keyword rules.
#[test]
fn verbatim_member_and_parameter_names_do_not_corrupt_scopes_or_kinds() {
    let src = r#"
        namespace Sample {
            public class Payload {
                public string @namespace { get; set; }
                public string @class { get; set; }
                public string @operator { get; set; }
                public int @event;
                public int @record(string @namespace, int @ref, int @in) {
                    int localInsideBody = 1;
                    return localInsideBody + @ref + @in;
                }
            }
        }
    "#;
    let scanned = scan(src);
    let types: Vec<&str> = scanned.types.iter().map(|t| t.full_name.as_str()).collect();
    assert_eq!(types, vec!["Sample.Payload"], "verbatim @namespace/@class/@record must not open fake scopes: {scanned:?}");

    let members = Members {
        types: vec![TypeInfo {
            token: "0x02000002".into(),
            full_name: "Sample.Payload".into(),
            namespace: "Sample".into(),
            name: "Payload".into(),
            kind: "class",
            visibility: "public",
            modifiers: vec![],
            extends: Some("System.Object".into()),
            enclosing_type: None,
            generic_params: vec![],
            interfaces: vec![],
            fields: vec![FieldInfo { token: "0x04000001".into(), name: "event".into(), ty: "System.Int32".into(), visibility: "public", modifiers: vec![] }],
            methods: vec![
                meta_method(".ctor", &[], "System.Void"),
                meta_method("get_namespace", &[], "System.String"),
                meta_method("set_namespace", &["System.String"], "System.Void"),
                meta_method("get_class", &[], "System.String"),
                meta_method("set_class", &["System.String"], "System.Void"),
                meta_method("get_operator", &[], "System.String"),
                meta_method("set_operator", &["System.String"], "System.Void"),
                meta_method("record", &["System.String", "System.Int32", "System.Int32"], "System.Int32"),
            ],
            properties: vec![
                PropertyInfo { name: "namespace".into(), ty: "System.String".into() },
                PropertyInfo { name: "class".into(), ty: "System.String".into() },
                PropertyInfo { name: "operator".into(), ty: "System.String".into() },
            ],
            events: vec![],
        }],
        total_types: 1,
        total_methods: 8,
        total_fields: 1,
        truncated: false,
        warnings: vec![],
    };

    let rec = reconcile(&members, &scanned);
    assert!(rec.missing_types.is_empty(), "{:?}", rec.missing_types);
    assert!(rec.extra_types.is_empty(), "{:?}", rec.extra_types);
    assert!(rec.missing.is_empty(), "missing metadata members: {:?}", rec.missing);
    assert!(rec.extra.is_empty(), "unexpected source members: {:?}", rec.extra);
    assert!(rec.signature_mismatch.is_empty(), "mismatched signatures: {:?}", rec.signature_mismatch);
}

/// Verbatim delegate, record, and type names (`@delegate`, `@struct`, `@event`, `@public`) scan as identifiers.
#[test]
fn verbatim_type_and_delegate_identifiers_are_not_treated_as_reserved_keywords() {
    let src = r#"
        namespace Sample {
            public delegate void @delegate(int @params);
            public record @struct(int @class);
            public class @public { }
            public class @event {
                public @event Next;
                public @public Owner;
            }
        }
    "#;
    let scanned = scan(src);
    let types: Vec<(&str, &str)> = scanned.types.iter().map(|t| (t.full_name.as_str(), t.kind.as_str())).collect();
    assert!(types.contains(&("Sample.delegate", "delegate")), "{types:?}");
    assert!(types.contains(&("Sample.struct", "record")), "{types:?}");
    assert!(types.contains(&("Sample.public", "class")), "{types:?}");
    assert!(types.contains(&("Sample.event", "class")), "{types:?}");

    assert!(
        scanned.members.iter().any(|m| m.type_name == "Sample.struct" && m.kind == "property" && m.name == "class"),
        "record `@struct(int @class)` must synthesize property `class`: {:?}",
        scanned.members
    );
    assert!(
        scanned.members.iter().any(|m| m.type_name == "Sample.event" && m.kind == "field" && m.name == "Next" && m.ty == "event"),
        "field of type `@event` must scan as field `Next`, not an untyped event: {:?}",
        scanned.members
    );
    assert!(
        scanned.members.iter().any(|m| m.type_name == "Sample.event" && m.kind == "field" && m.name == "Owner" && m.ty == "public"),
        "field of type `@public` must not have `@public` stripped as an access modifier: {:?}",
        scanned.members
    );
}

/// Verbatim `@checked` conversion targets keep type name `checked` instead of matching the `checked` operator keyword.
#[test]
fn verbatim_checked_conversion_target_keeps_type_name() {
    let src = r#"
        namespace Sample {
            public class @checked { }
            public class Box {
                public static explicit operator @checked(Box value) => null;
                public static implicit operator @checked(int value) => null;
            }
        }
    "#;
    let scanned = scan(src);
    assert!(
        scanned.members.iter().any(|m| m.type_name == "Sample.Box" && m.name == "op_Explicit" && m.ty == "checked"),
        "explicit conversion to `@checked` must scan as `op_Explicit` with target type `checked`: {:?}",
        scanned.members
    );
    assert!(
        scanned.members.iter().any(|m| m.type_name == "Sample.Box" && m.name == "op_Implicit" && m.ty == "checked"),
        "implicit conversion to `@checked` must scan as `op_Implicit` with target type `checked`: {:?}",
        scanned.members
    );
}

/// Non-verbatim `checked` operator keyword continues to produce `op_Checked*` operators.
#[test]
fn non_verbatim_checked_operator_keyword_scans_checked_operators() {
    let src = r#"
        namespace Sample {
            public class Box {
                public static explicit operator checked int(Box value) => 0;
                public static Box operator checked +(Box a, Box b) => a;
            }
        }
    "#;
    let scanned = scan(src);
    assert!(
        scanned.members.iter().any(|m| m.type_name == "Sample.Box" && m.name == "op_CheckedExplicit" && m.ty == "int"),
        "real `checked` explicit conversion must scan as `op_CheckedExplicit` with target type `int`: {:?}",
        scanned.members
    );
    assert!(
        scanned.members.iter().any(|m| m.type_name == "Sample.Box" && m.name == "op_CheckedAddition"),
        "real `checked` addition must scan as `op_CheckedAddition`: {:?}",
        scanned.members
    );
}
