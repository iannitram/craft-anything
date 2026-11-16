//! The analyzer's sourceMappingURL reader is the formats one: the standard gap after the marker,
//! the legacy `//@` and block forms, and no URL containing whitespace.

use craft_anything_js::{Limits, analyze};

fn url_at_end(source: &str) -> Option<String> {
    analyze("app.js", source, &Limits::default()).facts.source_map_url
}

/// Every marker and gap the formats reader accepts is found through `analyze`.
#[test]
fn every_standard_annotation_form_is_found() {
    for marker in ["//#", "//@", "/*#"] {
        for gap in ["", " ", "\t", "  \t"] {
            let end = if marker.starts_with("/*") { " */" } else { "" };
            let source = format!("main();\n{marker}{gap}sourceMappingURL=app.js.map{end}\n");
            assert_eq!(url_at_end(&source).as_deref(), Some("app.js.map"), "{marker}{gap:?}");
        }
    }
}

/// A URL containing whitespace, or no URL at all, is not a source map.
#[test]
fn a_url_with_whitespace_is_not_a_source_map() {
    assert_eq!(url_at_end("main();\n//# sourceMappingURL=two words.map\n"), None);
    assert_eq!(url_at_end("main();\n//# sourceMappingURL=\n"), None);
}

/// The shared reader looks further back than the eight lines the JS copy used to scan; an
/// annotation followed by code is still found.
#[test]
fn an_annotation_before_trailing_code_is_found() {
    let source = format!("//#sourceMappingURL=app.js.map\n{}", "main();\n".repeat(20));
    assert_eq!(url_at_end(&source).as_deref(), Some("app.js.map"));
}
