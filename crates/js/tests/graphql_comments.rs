use craft_anything_js::api_ops::read_document;

/// Header comments must not consume the operation name or change bracket depth.
#[test]
fn comments_can_separate_header_tokens() {
    for text in ["query # note\nQ { a }", "query Q # ( { fake\n{ a }", "query Q($x: Int # ) {\n) { a }", "query\r# ignored\rQ { a }", "query,\u{feff}Q { a }"] {
        let doc = read_document(text, true).expect(text);
        assert_eq!(doc.operations[0].name.as_deref(), Some("Q"), "{text}");
    }
}

/// Comments never create a missing selection set; quoted hash marks stay strings.
#[test]
fn retains_header_validation_and_string_contents() {
    assert!(read_document("query Q # { fake }", true).is_none());
    assert!(read_document("query Q($x: Int # comment\n { a }", true).is_none());
    let doc = read_document("query Q($x: String = \"# ( {\") { a }", true).unwrap();
    assert_eq!(doc.operations[0].name.as_deref(), Some("Q"));
}
