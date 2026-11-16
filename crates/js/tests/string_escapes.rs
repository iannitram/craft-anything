use craft_anything_js::lex::unescape;

/// Annex B octal escapes consume only the digits allowed by the leading digit.
#[test]
fn decodes_legacy_octal_escapes() {
    for (input, output) in [(r"\141", "a"), (r"\012", "\n"), (r"\377", "ÿ"), (r"\400", " 0"), (r"\777", "?7"), (r"\08", "\08"), (r"\8\9", "89")] {
        assert_eq!(unescape(input), output, "{input}");
    }
}

/// All four line terminators disappear after a continuation backslash.
#[test]
fn decodes_unicode_line_continuations() {
    for term in ["\n", "\r", "\r\n", "\u{2028}", "\u{2029}"] {
        assert_eq!(unescape(&format!("a\\{term}b")), "ab");
    }
}

/// Braced escapes permit leading zeroes and leave following text untouched.
#[test]
fn decodes_zero_padded_unicode() {
    assert_eq!(unescape(r"\u{000000041}Z"), "AZ");
    assert_eq!(unescape(r"\u{0000001f600}Z"), "😀Z");
    assert_eq!(unescape(&format!("\\u{{{}41}}Z", "0".repeat(10000))), "AZ");
}

/// Existing escape forms and surrogate pairs retain their values.
#[test]
fn keeps_common_escape_values() {
    assert_eq!(unescape(r"\x41\u0042\u{43}\uD83D\uDE00"), "ABC😀");
    assert_eq!(unescape(r"\0\n\t\\"), "\0\n\t\\");
}

/// The fallback inventory uses decoded paths, not the source spelling of escapes.
#[test]
fn fallback_imports_and_endpoints_use_decoded_values() {
    use craft_anything_js::{Limits, analyze};
    let source = r#"require('./\141pp.js'); fetch('/\u{000000061}pi');"#;
    let a = analyze("app.js", source, &Limits { max_parse_bytes: 0, ..Limits::default() });
    assert!(a.facts.imports.iter().any(|i| i.specifier == "./app.js"));
    assert!(a.facts.endpoints.iter().any(|e| e.api == "fetch" && e.url == "/api"));
}
