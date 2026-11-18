use craft_anything_js::html::scan_html;

/// Collect all declared policies in document order and keep the textual evidence field.
#[test]
fn retains_both_meta_content_security_policies() {
    let html = r#"<meta http-equiv="Content-Security-Policy" content="default-src 'self'">
<meta http-equiv="content-security-policy" content="script-src 'none'">"#;
    assert_eq!(scan_html(html).csp.as_deref(), Some("default-src 'self', script-src 'none'"));
}

/// Empty or missing policies do not erase a preceding usable policy.
#[test]
fn empty_meta_policies_do_not_erase_preceding_evidence() {
    let html = r#"<meta http-equiv="content-security-policy" content="object-src 'none'">
<meta http-equiv="content-security-policy" content=" ">
<meta http-equiv="content-security-policy">
<meta http-equiv="content-security-policy-report-only" content="default-src *">"#;
    assert_eq!(scan_html(html).csp.as_deref(), Some("object-src 'none'"));
}
