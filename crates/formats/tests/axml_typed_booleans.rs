//! Regression tests for integer-typed boolean attributes and unnamespaced `package` in AXML manifests.

use craft_anything_formats::axml::{Attribute, Document, Node, ResValue, manifest, parse};
use craft_anything_testkit::android::{AxAttr, AxNode, AxVal, a, axml};

/// Standard `0x12` (`TYPE_INT_BOOLEAN`) attributes continue to decode as expected.
#[test]
fn boolean_type_0x12_attributes_decode_normally() {
    let raw = axml(&[
        AxNode::Start("manifest", vec![AxAttr { name: "package", res_id: None, val: AxVal::Str("com.example.app") }]),
        AxNode::Start("uses-sdk", vec![a("targetSdkVersion", 0x0101_0270, AxVal::Int(30))]),
        AxNode::End("uses-sdk"),
        AxNode::Start("application", vec![a("debuggable", 0x0101_000f, AxVal::Bool(true))]),
        AxNode::Start("activity", vec![a("name", 0x0101_0003, AxVal::Str(".MainActivity")), a("exported", 0x0101_0010, AxVal::Bool(false))]),
        AxNode::Start("intent-filter", vec![]),
        AxNode::End("intent-filter"),
        AxNode::End("activity"),
        AxNode::End("application"),
        AxNode::End("manifest"),
    ]);
    let doc = parse(&raw).unwrap();
    let m = manifest(&doc);
    assert_eq!(m.package.as_deref(), Some("com.example.app"));
    assert_eq!(m.application.debuggable, Some(true));
    assert_eq!(m.components[0].exported_attribute, Some(false));
    assert!(!m.components[0].exported);
}

/// Integer-typed `Res_value` (`0x10` decimal, `0x11` hex) booleans must not be dropped as `None`.
#[test]
fn integer_typed_res_values_decode_as_manifest_booleans() {
    let raw = axml(&[
        AxNode::Start("manifest", vec![AxAttr { name: "package", res_id: None, val: AxVal::Str("com.example.app") }]),
        AxNode::Start("uses-sdk", vec![a("targetSdkVersion", 0x0101_0270, AxVal::Int(29))]),
        AxNode::End("uses-sdk"),
        AxNode::Start(
            "application",
            vec![
                a("debuggable", 0x0101_000f, AxVal::Int(1)),
                a("allowBackup", 0x0101_0280, AxVal::Int(0)),
                a("usesCleartextTraffic", 0x0101_04ec, AxVal::Hex(0xffff_ffff)),
            ],
        ),
        // Activity has an `<intent-filter>` on API 29, which implies `exported = true` UNLESS
        // `android:exported` is explicitly present. Encoding `exported = 0` as TYPE_INT_DEC (0x10)
        // or TYPE_INT_HEX (0x11) must keep `exported: false`.
        AxNode::Start("activity", vec![a("name", 0x0101_0003, AxVal::Str(".InternalActivity")), a("exported", 0x0101_0010, AxVal::Int(0))]),
        AxNode::Start("intent-filter", vec![]),
        AxNode::End("intent-filter"),
        AxNode::End("activity"),
        AxNode::Start("provider", vec![a("name", 0x0101_0003, AxVal::Str(".LocalProvider")), a("exported", 0x0101_0010, AxVal::Hex(0))]),
        AxNode::End("provider"),
        AxNode::End("application"),
        AxNode::End("manifest"),
    ]);
    let doc = parse(&raw).unwrap();
    let m = manifest(&doc);
    assert_eq!(m.application.debuggable, Some(true));
    assert_eq!(m.application.allow_backup, Some(false));
    assert_eq!(m.application.uses_cleartext_traffic, Some(true));

    assert_eq!(m.components[0].exported_attribute, Some(false));
    assert!(!m.components[0].exported, "explicit integer `exported=0` must override intent-filter default");
    assert_eq!(m.components[1].exported_attribute, Some(false));
    assert!(!m.components[1].exported);
}

/// A namespaced `package` attribute on `<manifest>` must not shadow the unnamespaced `package` attribute.
#[test]
fn namespaced_package_attribute_does_not_shadow_manifest_package() {
    let doc = Document {
        nodes: vec![
            Node::Start {
                depth: 0,
                name: "manifest".into(),
                namespace: None,
                line: 1,
                attributes: vec![
                    Attribute {
                        namespace: Some("http://schemas.android.com/tools".into()),
                        name: "package".into(),
                        resource_id: None,
                        value: "com.wrong.shadow".into(),
                        value_type: "string",
                        raw: ResValue { data_type: 0x03, data: 0 },
                    },
                    Attribute {
                        namespace: None,
                        name: "package".into(),
                        resource_id: None,
                        value: "com.real.app".into(),
                        value_type: "string",
                        raw: ResValue { data_type: 0x03, data: 1 },
                    },
                ],
            },
            Node::Start { depth: 1, name: "application".into(), namespace: None, line: 2, attributes: vec![] },
            Node::Start {
                depth: 2,
                name: "activity".into(),
                namespace: None,
                line: 3,
                attributes: vec![Attribute {
                    namespace: Some("http://schemas.android.com/apk/res/android".into()),
                    name: "name".into(),
                    resource_id: None,
                    value: ".HomeActivity".into(),
                    value_type: "string",
                    raw: ResValue { data_type: 0x03, data: 2 },
                }],
            },
            Node::End { depth: 2, name: "activity".into() },
            Node::End { depth: 1, name: "application".into() },
            Node::End { depth: 0, name: "manifest".into() },
        ],
        ..Document::default()
    };
    let m = manifest(&doc);
    assert_eq!(m.package.as_deref(), Some("com.real.app"));
    assert_eq!(m.components[0].name, "com.real.app.HomeActivity");
}
