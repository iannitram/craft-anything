use craft_anything_formats::axml::{manifest, parse};
use craft_anything_testkit::android::{AxNode, AxVal, a, axml};

/// All platform-recognized permission declarations go into the manifest inventory.
#[test]
fn keeps_the_legacy_sdk_m_permission_tag() {
    let bytes = axml(&[
        AxNode::Start("manifest", vec![]),
        AxNode::Start("uses-permission", vec![a("name", 0x0101_0003, AxVal::Str("android.permission.INTERNET"))]),
        AxNode::End("uses-permission"),
        AxNode::Start("uses-permission-sdk-23", vec![a("name", 0x0101_0003, AxVal::Str("android.permission.CAMERA"))]),
        AxNode::End("uses-permission-sdk-23"),
        AxNode::Start("uses-permission-sdk-m", vec![a("name", 0x0101_0003, AxVal::Str("android.permission.RECORD_AUDIO"))]),
        AxNode::End("uses-permission-sdk-m"),
        AxNode::Start("uses-permission-sdk-m", vec![]),
        AxNode::End("uses-permission-sdk-m"),
        AxNode::End("manifest"),
    ]);
    let result = manifest(&parse(&bytes).unwrap());
    assert_eq!(result.uses_permissions, ["android.permission.INTERNET", "android.permission.CAMERA", "android.permission.RECORD_AUDIO"]);
}
