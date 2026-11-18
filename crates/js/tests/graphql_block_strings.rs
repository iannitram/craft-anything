use craft_anything_js::api_ops::read_document;

/// The selection and later definitions survive escaped triple quotes in an argument.
#[test]
fn keeps_operations_after_an_escaped_graphql_block_delimiter() {
    let text = r#"query Search { item(text: """Before \""" } # still text \""" after""") { id } } query Next { id }"#;
    let document = read_document(text, false).unwrap();
    let names: Vec<_> = document.operations.iter().filter_map(|operation| operation.name.as_deref()).collect();
    assert_eq!(names, ["Search", "Next"]);
}

/// A variable's default block string can contain escaped quotes and fake header punctuation.
#[test]
fn ignores_escaped_graphql_delimiters_in_variable_defaults() {
    let text = r#"query Search($input: String = """\""" ) { text""") { search(text: $input) }"#;
    let document = read_document(text, false).unwrap();
    assert_eq!(document.operations[0].name.as_deref(), Some("Search"));
}

/// An escaped delimiter followed by EOF is still an unterminated block string.
#[test]
fn rejects_an_unterminated_graphql_block_string() {
    assert!(read_document(r#"query Search { item(text: """\""""#, false).is_none());
}
