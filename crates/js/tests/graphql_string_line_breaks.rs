use craft_anything_js::api_ops::read_document;

/// A raw CR invalidates quoted variable defaults and arguments.
#[test]
fn rejects_carriage_returns_in_regular_strings() {
    for text in ["query Q($s: String = \"a\rb\") { item(s: $s) }", "query Q { item(s: \"a\rb\") }"] {
        assert!(read_document(text, false).is_none(), "{text:?}");
    }
}

/// A backslash cannot continue a regular GraphQL string onto the next line.
#[test]
fn rejects_physical_line_breaks_after_backslashes() {
    for newline in ["\n", "\r", "\r\n"] {
        let text = format!("query Q {{ item(s: \"a\\{newline}b\") }}");
        assert!(read_document(&text, false).is_none(), "{text:?}");
    }
}

/// Textual escapes and actual line breaks in block strings stay valid.
#[test]
fn keeps_escape_sequences_and_multiline_block_strings() {
    for text in [r#"query Q { item(s: "a\rb\nc") }"#, "query Q { item(s: \"\"\"a\rb\nc\"\"\") }"] {
        assert_eq!(read_document(text, false).unwrap().operations[0].name.as_deref(), Some("Q"));
    }
}
