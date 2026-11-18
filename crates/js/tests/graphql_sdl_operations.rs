//! Regression tests for GraphQL documents mixing SDL definitions or descriptions with operations.

use craft_anything_js::api_ops::{may_be_document, read_document};
use craft_anything_js::{Limits, analyze};

/// Documents with braced `type` or `fragment` definitions before operations continue to extract operations.
#[test]
fn braced_type_and_fragment_definitions_preceding_query_work() {
    let doc = read_document("type User { id: ID! }\nfragment Fields on User { id }\nquery GetUser { user { ...Fields } }", false).unwrap();
    assert_eq!(doc.operations.len(), 1);
    assert_eq!(doc.operations[0].kind, "query");
    assert_eq!(doc.operations[0].name.as_deref(), Some("GetUser"));
}

/// Braceless SDL definitions (`scalar`, `union`, `directive`, `extend`) must not swallow following operations.
#[test]
fn braceless_sdl_definitions_do_not_swallow_operations() {
    let cases = [
        ("scalar DateTime\nquery GetFeed { feed { createdAt } }", "query", Some("GetFeed")),
        (
            "scalar JSON @specifiedBy(url: \"https:// scalars.graphql.org/andimarek/json\")\nmutation Save($p: JSON!) { save(payload: $p) }",
            "mutation",
            Some("Save"),
        ),
        ("union SearchResult = User | Post\nquery Search($q: String!) { search(q: $q) { __typename } }", "query", Some("Search")),
        (
            "directive @client(always: Boolean = false) repeatable on FIELD | FRAGMENT_SPREAD\nquery LocalState { me @client { id } }",
            "query",
            Some("LocalState"),
        ),
        (
            "extend scalar DateTime @tag(name: \"iso\")\nextend union SearchResult = Comment\nextend schema @link(url: \"https://specs.apollo.dev/federation/v2.0\")\nquery Fed { me { id } }",
            "query",
            Some("Fed"),
        ),
    ];

    for (src, expected_kind, expected_name) in cases {
        assert!(may_be_document(src), "may_be_document should accept mixed SDL + operation:\n{src}");
        let doc = read_document(src, false).unwrap_or_else(|| panic!("expected valid GraphQL document:\n{src}"));
        assert_eq!(doc.operations.len(), 1, "expected 1 operation in:\n{src}\ngot: {doc:?}");
        assert_eq!(doc.operations[0].kind, expected_kind);
        assert_eq!(doc.operations[0].name.as_deref(), expected_name);
    }

    // Trailing braceless SDL after an operation must also remain a valid document.
    let trailing = "query GetUser { user { id } }\nscalar DateTime\ndirective @client on FIELD";
    let doc = read_document(trailing, false).expect("trailing SDL definitions are valid in a GraphQL document");
    assert_eq!(doc.operations.len(), 1);
    assert_eq!(doc.operations[0].name.as_deref(), Some("GetUser"));
}

/// Leading GraphQL `StringValue` descriptions (`"..."` and `"""..."""`) on definitions are accepted.
#[test]
fn leading_graphql_descriptions_are_accepted_without_loosening_prose_rejection() {
    let described = "\"\"\"\nFetches the signed-in account.\n\"\"\"\nquery CurrentAccount { viewer { id } }";
    assert!(may_be_document(described));
    let doc = read_document(described, false).expect("block-string description before query is valid GraphQL");
    assert_eq!(doc.operations.len(), 1);
    assert_eq!(doc.operations[0].name.as_deref(), Some("CurrentAccount"));

    let described_sdl = "\"ISO-8601 timestamp\"\nscalar DateTime\n\"Account lookup\"\nmutation UpdateTime($t: DateTime!) { setTime(t: $t) }";
    assert!(may_be_document(described_sdl));
    let doc2 = read_document(described_sdl, false).expect("single-line descriptions in mixed document are valid");
    assert_eq!(doc2.operations.len(), 1);
    assert_eq!(doc2.operations[0].name.as_deref(), Some("UpdateTime"));

    // Plain English prose or non-GraphQL strings starting with SDL words or quotes must still be rejected.
    for non_graphql in [
        "scalar the mountain { fast }",
        "union the workers { now }",
        "directive to all hands { read }",
        "extend the warranty { days }",
        "\"just a quoted string with { braces }\"",
        "scalar",
        "union A =",
        "directive @client",
    ] {
        assert!(read_document(non_graphql, false).is_none(), "must reject non-GraphQL text: {non_graphql}");
    }
}

/// End-to-end JS analysis recovers GraphQL operations from both string literals and `gql` templates with SDL headers.
#[test]
fn js_analysis_extracts_operations_from_sdl_prefixed_documents() {
    let js = r#"
        const A = gql`
            directive @client on FIELD
            query ReadCart { cart @client { total } }
        `;
        const B = "scalar Upload\nmutation UploadAvatar($f: Upload!) { uploadAvatar(file: $f) { ok } }";
    "#;
    let out = analyze("app.js", js, &Limits::default());
    let ops: Vec<(&str, Option<&str>)> = out.facts.api_operations.iter().map(|o| (o.kind.as_str(), o.name.as_deref())).collect();
    assert!(ops.contains(&("query", Some("ReadCart"))), "{ops:?}");
    assert!(ops.contains(&("mutation", Some("UploadAvatar"))), "{ops:?}");
}

/// GraphQL allows ignored tokens (whitespace, comments, commas) between `@` and a directive name.
#[test]
fn directive_at_sign_allows_ignored_tokens_before_name() {
    let cases = [("scalar Date @ specifiedBy(url: \"urn:date\")\nquery Q { a }", "Q"), ("directive @ # comment\n , client on FIELD\nquery Q2 { a }", "Q2")];
    for (src, expected_name) in cases {
        assert!(may_be_document(src), "may_be_document should accept spaced directive:\n{src}");
        let doc = read_document(src, false).unwrap_or_else(|| panic!("expected valid GraphQL document:\n{src}"));
        assert_eq!(doc.operations.len(), 1, "expected 1 operation in:\n{src}\ngot: {doc:?}");
        assert_eq!(doc.operations[0].name.as_deref(), Some(expected_name));
    }
}

/// Adjacent `@client` and `@specifiedBy` directives continue to parse alongside operations.
#[test]
fn adjacent_directive_at_sign_continues_to_parse() {
    let src = "scalar Date @specifiedBy(url: \"urn:date\")\ndirective @client on FIELD\nquery KeepAdjacent { me @client { id } }";
    let doc = read_document(src, false).expect("adjacent @directive forms must remain valid");
    assert_eq!(doc.operations.len(), 1);
    assert_eq!(doc.operations[0].kind, "query");
    assert_eq!(doc.operations[0].name.as_deref(), Some("KeepAdjacent"));
}
