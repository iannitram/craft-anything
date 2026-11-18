use craft_anything_js::{Limits, analyze};

/// Both fact readers retain the literal specifier of an import with attributes.
#[test]
fn keeps_dynamic_imports_with_options_in_fallback() {
    let source = r#"import("./data.json", { with: { type: "json" } });"#;
    for limits in [Limits::default(), Limits { max_parse_bytes: 0, ..Limits::default() }] {
        let facts = analyze("entry.js", source, &limits).facts;
        assert_eq!(facts.imports.len(), 1);
        assert_eq!(facts.imports[0].kind, "dynamic-import");
        assert_eq!(facts.imports[0].specifier, "./data.json");
        assert!(facts.imports[0].literal);
    }
}

/// The trailing comma form has the same literal specifier as a plain import call.
#[test]
fn keeps_the_trailing_comma_form() {
    let facts = analyze("entry.js", r#"import("./plain.js",);"#, &Limits { max_parse_bytes: 0, ..Limits::default() }).facts;
    assert_eq!(facts.imports.len(), 1);
    assert_eq!(facts.imports[0].specifier, "./plain.js");
}

/// The string prefix of a computed specifier is not itself an import.
#[test]
fn leaves_computed_specifiers_and_require_argument_rules_alone() {
    let source = r#"import("./prefix-" + name, opts); require("./one.node"); require("./extra.node", opts); loader.import("./method.js", opts); loader?.import("./optional.js", opts);"#;
    let facts = analyze("entry.js", source, &Limits { max_parse_bytes: 0, ..Limits::default() }).facts;
    assert_eq!(facts.imports.iter().map(|import| import.specifier.as_str()).collect::<Vec<_>>(), ["./one.node"]);
}
