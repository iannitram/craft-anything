use craft_anything_js::html::scan_html;

/// Padded keyword values do not run as either classic scripts or modules.
#[test]
fn ignores_padded_module_keywords_and_whitespace_only_types() {
    for script_type in [" module ", "\tmodule", "module&#10;", " \t "] {
        let html = format!(r#"<script type="{script_type}" src="/ignored.js"></script>"#);
        assert!(scan_html(&html).scripts.is_empty(), "{script_type:?}");
    }
}

/// The exact module keyword and a whitespace-padded classic MIME type are still recognized.
#[test]
fn keeps_exact_module_and_trimmed_classic_mime_types() {
    let html = r#"<script type="MoDuLe" src="/module.js"></script>
<script type=" text/javascript " src="/classic.js"></script>
<script type="" src="/default.js"></script>"#;
    let facts = scan_html(html);
    assert_eq!(facts.scripts.len(), 3);
    assert!(facts.scripts[0].module);
    assert!(facts.scripts[1..].iter().all(|script| !script.module));
}
