use craft_anything_js::html::scan_html;

/// Malformed declarations and end tags consume apparent markup through the next greater-than sign.
#[test]
fn skips_tags_inside_bogus_comments() {
    let facts = scan_html(concat!(
        "<!doctype html><!unknown <script src=ghost.js></script>",
        "</?bad <link rel=modulepreload href=ghost.mjs>",
        "<script src=app.js></script><link rel=modulepreload href=app.mjs>",
    ));
    let scripts: Vec<_> = facts.scripts.iter().filter_map(|s| s.src.as_deref()).collect();
    assert_eq!(scripts, ["app.js"]);
    assert_eq!(facts.preloads, ["app.mjs"]);
}

/// A malformed declaration that reaches EOF must not supply a script.
#[test]
fn skips_unterminated_bogus_declaration() {
    assert!(scan_html("<!unknown <script src='ghost.js'").scripts.is_empty());
}

/// A bare less-than sign reconsumes the following character in HTML data.
#[test]
fn keeps_a_script_after_a_literal_less_than_sign() {
    let facts = scan_html("<<script src=app.js></script>");
    assert_eq!(facts.scripts[0].src.as_deref(), Some("app.js"));
}
