use craft_anything_js::html::scan_html;

/// HTML resumes after the browser's alternate comment terminators, including empty comments.
#[test]
fn keeps_scripts_after_html_comments_that_close() {
    for comment in ["<!-->", "<!--->", "<!-- hidden <script src=wrong.js></script> --!>", "<!-- hidden -->"] {
        let html = format!("{comment}<script src=app.js></script>");
        let facts = scan_html(&html);
        assert_eq!(facts.scripts.len(), 1, "{comment}");
        assert_eq!(facts.scripts[0].src.as_deref(), Some("app.js"));
    }
}

/// A spurious bang and an unfinished comment still hide their contents.
#[test]
fn keeps_embedded_tags_hidden_until_the_html_comment_closes() {
    let html = "<!-- hidden -!> <script src=wrong.js></script> --!><link rel=modulepreload href=right.js><!-- <script src=missing.js>";
    let facts = scan_html(html);
    assert!(facts.scripts.is_empty());
    assert_eq!(facts.preloads, ["right.js"]);
}
