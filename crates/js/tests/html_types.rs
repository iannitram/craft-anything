use craft_anything_js::html::scan_html;

/// Unsupported suffixes and parameters describe data, not executable scripts.
#[test]
fn rejects_script_type_lookalikes() {
    for kind in [
        "application/javascript123",
        "text/javascript1garbage",
        "text/javascript1.6",
        "application/javascript1.0",
        "module; charset=utf-8",
        "text/javascript; charset=utf-8",
        ";",
    ] {
        let html = format!(r#"<script type="{kind}" src="data.js"></script><script type="{kind}">fetch('/data')</script>"#);
        assert!(scan_html(&html).scripts.is_empty(), "{kind}");
    }
}

/// All historically recognized numbered text types remain executable.
#[test]
fn keeps_supported_script_types() {
    for kind in [
        "",
        "module",
        "text/javascript",
        "Application/JavaScript",
        "text/javascript1.0",
        "text/javascript1.1",
        "text/javascript1.2",
        "text/javascript1.3",
        "text/javascript1.4",
        "text/javascript1.5",
        "text/babel",
        "text/jsx",
    ] {
        assert_eq!(scan_html(&format!(r#"<script type="{kind}" src="app.js"></script>"#)).scripts.len(), 1, "{kind}");
    }
}
