use craft_anything_js::html::scan_html;

/// Legacy language values that do not select a JavaScript MIME type add no script evidence.
#[test]
fn omits_unsupported_legacy_languages() {
    let html = r#"<script language="VBScript" src="/legacy.vbs"></script>
<script language="module">ignored()</script>
<script language="Babel" src="/legacy.jsx"></script>
<script language=" JavaScript" src="/leading-space.js"></script>
<script language="JavaScript&#32;" src="/trailing-space.js"></script>"#;
    assert!(scan_html(html).scripts.is_empty());
}

/// Standard classic legacy names and the default still work.
#[test]
fn keeps_classic_legacy_languages_and_empty_language() {
    let html = r#"<script language="JaVaScRiPt" src="/legacy.js"></script>
<script language="JavaScript1.2" src="/version.js"></script>
<script language="JScript" src="/jscript.js"></script>
<script language="" src="/default.js"></script>"#;
    let facts = scan_html(html);
    let scripts: Vec<_> = facts.scripts.iter().map(|s| (s.src.as_deref().unwrap(), s.module)).collect();
    assert_eq!(scripts, [("/legacy.js", false), ("/version.js", false), ("/jscript.js", false), ("/default.js", false)]);
}

/// An explicit type overrides all legacy language values, even when the type is empty.
#[test]
fn honors_explicit_type_over_legacy_language() {
    let html = r#"<script type="module" language="VBScript" src="/module.js"></script>
<script type="" language="VBScript" src="/classic.js"></script>
<script type="text/babel" language="VBScript" src="/explicit.jsx"></script>
<script type="application/json" language="JavaScript" src="/data.json"></script>"#;
    let facts = scan_html(html);
    let scripts: Vec<_> = facts.scripts.iter().map(|s| (s.src.as_deref().unwrap(), s.module)).collect();
    assert_eq!(scripts, [("/module.js", true), ("/classic.js", false), ("/explicit.jsx", false)]);
}
