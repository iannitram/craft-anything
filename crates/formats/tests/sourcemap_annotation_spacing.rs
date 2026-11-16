use craft_anything_formats::sourcemap::find_source_mapping_url;

/// Standard annotations permit zero or several whitespace characters after the marker.
#[test]
fn annotation_marker_accepts_standard_whitespace() {
    for marker in ["//#", "//@", "/*#"] {
        for gap in ["", " ", "\t", "  \t"] {
            let end = if marker.starts_with("/*") { " */" } else { "" };
            assert_eq!(find_source_mapping_url(&format!("{marker}{gap}sourceMappingURL=app.js.map{end}")), Some("app.js.map".into()));
        }
    }
    assert_eq!(find_source_mapping_url("//# sourceMappingURL=two words.map"), None);
    assert_eq!(find_source_mapping_url("//# other=app.js.map"), None);
}
