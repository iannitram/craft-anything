use craft_anything_js::{Limits, analyze, vendor::banner_vendor};
/// Attached versions must not erase the npm scope.
#[test]
fn scoped_attached_versions() {
    for name in ["@scope/pkg", "@other/pkg"] {
        let v = banner_vendor(&format!("/*! {name}@1.2.3 */"), 7).unwrap();
        assert_eq!(v.name, name);
        assert_eq!(v.version.as_deref(), Some("1.2.3"));
        assert_eq!(v.line, 7);
    }
}
/// Spaced versions and license markers preserve the same scoped identity.
#[test]
fn scoped_spaced_versions() {
    for c in ["/*! @scope/pkg v1.2.3 */", "/** @license @scope/pkg v1.2.3 */", "/*! @scope/pkg version 1.2.3 */"] {
        let v = banner_vendor(c, 1).unwrap();
        assert_eq!(v.name, "@scope/pkg");
        assert_eq!(v.version.as_deref(), Some("1.2.3"));
    }
}
/// Parsed and fallback analysis both preserve the banner's complete package name.
#[test]
fn analyzer_preserves_scoped_banner() {
    for limits in [Limits::default(), Limits { max_parse_bytes: 0, ..Limits::default() }] {
        let a = analyze("app.js", "/*! @scope/pkg@1.2.3 */\nconst x = 1;", &limits);
        assert!(a.facts.vendors.iter().any(|v| v.name == "@scope/pkg"));
    }
}
/// Unscoped names and non-banner comments retain their previous behavior.
#[test]
fn ordinary_banners_are_unchanged() {
    for c in ["/*! axios@1.6.0 */", "/*! axios v1.6.0 */"] {
        assert_eq!(banner_vendor(c, 1).unwrap().name, "axios");
    }
    assert!(banner_vendor("// ordinary comment v1.2.3", 1).is_none());
}
/// A banner that names its package by URL still splits the URL on `/`.
#[test]
fn url_banners_split_on_slashes() {
    for (c, name, version) in [
        ("/*! https://mths.be/punycode v1.4.1 by @mathias */", "punycode", "1.4.1"),
        ("/*! https://unpkg.com/axios@1.6.0/dist/axios.min.js */", "axios", "1.6.0"),
    ] {
        let v = banner_vendor(c, 1).unwrap();
        assert_eq!((v.name.as_str(), v.version.as_deref()), (name, Some(version)), "{c}");
    }
}
