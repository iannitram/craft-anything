use craft_anything_js::html::scan_html;

/// An unfinished start tag cannot supply a script, even if a quoted attribute contains '>'.
#[test]
fn drops_incomplete_script_start_tags() {
    for text in [r#"<script src="app.js""#, r#"<script src=app.js"#, r#"<script src="app.js" data-note=">"#] {
        assert!(scan_html(text).scripts.is_empty(), "{text}");
    }
}

/// A complete earlier preload survives a truncated tag at the end.
#[test]
fn keeps_real_preloads_before_a_truncated_link() {
    let facts = scan_html(r#"<link rel="modulepreload" href="/real.mjs"><link rel=modulepreload href="/ghost.mjs""#);
    assert_eq!(facts.preloads, ["/real.mjs"]);
}

/// A cut-off meta tag or title does not supply page metadata.
#[test]
fn drops_incomplete_page_metadata_tags() {
    assert!(scan_html("<meta http-equiv=content-security-policy content='default-src self'").csp.is_none());
    assert!(scan_html("<title").title.is_none());
    assert_eq!(scan_html("<title>Ready</title>").title.as_deref(), Some("Ready"));
}
