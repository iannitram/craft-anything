//! Regression tests for lexer-fallback list caps, URL endpoint deduplication, and `truncated`
//! reporting in `fallback::facts_from_tokens`.

use craft_anything_js::{Limits, ParseMode, analyze};
use std::fmt::Write;

/// Limits that force every source onto the lexer fallback path.
fn lexer_limits() -> Limits {
    Limits { max_parse_bytes: 0, ..Limits::default() }
}

/// `fetch("https://...")` is not recorded a second time as a `literal` endpoint (including at the 240-byte URL boundary), repeated literal URLs deduplicate, and a literal URL before `fetch(...)` matches syntax-tree order.
#[test]
fn fetch_and_repeated_literal_urls_do_not_duplicate_endpoints_in_lexer_mode() {
    let exact_240_url = format!("https://ex.test/{}", "a".repeat(224));
    assert_eq!(exact_240_url.len(), 240);
    let src = format!(
        r#"
        fetch("https://api.example.test/v1/items");
        fetch("{exact_240_url}");
        const a = "https://cdn.example.test/app.js";
        const b = "https://cdn.example.test/app.js";
        const first = "https://api.example.test/v1/later-fetched";
        fetch("https://api.example.test/v1/later-fetched");
    "#
    );

    let parsed = analyze("app.js", &src, &Limits::default()).facts;
    let lexed = analyze("app.js", &src, &lexer_limits()).facts;
    assert_eq!(lexed.mode, ParseMode::Lexer);
    assert_eq!(parsed.endpoints.len(), 5);
    assert_eq!(lexed.endpoints.len(), 5, "lexer endpoints: {:?}", lexed.endpoints);
    assert_eq!(lexed.endpoints[0].api, "fetch");
    assert_eq!(lexed.endpoints[0].url, "https://api.example.test/v1/items");
    assert_eq!(lexed.endpoints[1].api, "fetch");
    assert_eq!(lexed.endpoints[1].url, exact_240_url);
    assert_eq!(lexed.endpoints[2].api, "literal");
    assert_eq!(lexed.endpoints[2].url, "https://cdn.example.test/app.js");
    assert_eq!(lexed.endpoints[3].api, "literal");
    assert_eq!(lexed.endpoints[3].url, "https://api.example.test/v1/later-fetched");
    assert_eq!(lexed.endpoints[4].api, "fetch");
    assert_eq!(lexed.endpoints[4].url, "https://api.example.test/v1/later-fetched");
}

/// Literal URL endpoints still record after `MAX_STRINGS` (4,000) unique plain strings, and `strings` marks truncated only above 4,000 unique entries.
#[test]
fn literal_urls_survive_past_max_strings_and_strings_truncation_checks_exact_cap() {
    let mut exact = String::new();
    for i in 0..4_000 {
        let _ = writeln!(exact, "const s{i} = \"plain_str_{i}\";");
    }
    exact.push_str("const again = \"plain_str_0\";\n");

    let exact_facts = analyze("exact.js", &exact, &lexer_limits()).facts;
    assert_eq!(exact_facts.strings.len(), 4_000);
    assert!(!exact_facts.truncated.contains(&"strings".to_string()), "exact cap + duplicate must not mark truncated: {:?}", exact_facts.truncated);

    let mut over = exact;
    over.push_str("const late = \"https://api.example.test/after-string-cap\";\n");
    let over_facts = analyze("over.js", &over, &lexer_limits()).facts;
    assert_eq!(over_facts.strings.len(), 4_000);
    assert!(over_facts.truncated.contains(&"strings".to_string()), "4001st unique string must mark strings truncated");
    assert!(
        over_facts.endpoints.iter().any(|e| e.url == "https://api.example.test/after-string-cap"),
        "literal URL after MAX_STRINGS must still enter endpoints: {:?}",
        over_facts.endpoints
    );
}

/// `.node` native module imports use `MAX_LIST` (5,000) rather than truncating silently at 1,000.
#[test]
fn native_node_modules_cap_matches_syntax_tree_max_list() {
    let mut src = String::new();
    for i in 0..5_000 {
        let _ = writeln!(src, "require(\"native_{i}.node\");");
    }
    let exact = analyze("addons_exact.js", &src, &lexer_limits()).facts;
    assert_eq!(exact.native_modules.len(), 5_000);
    assert!(!exact.truncated.contains(&"imports".to_string()));

    src.push_str("require(\"native_5000.node\");\n");
    let over = analyze("addons_over.js", &src, &lexer_limits()).facts;
    assert_eq!(over.native_modules.len(), 5_000);
    assert!(over.truncated.contains(&"imports".to_string()));
}

/// `ipc`, `exposed`, and literal `endpoints` mark `truncated` at `MAX_LIST + 1` (5,001) while staying untruncated at 5,000 (even when a duplicate URL appears at cap).
#[test]
fn ipc_exposed_and_literal_endpoints_report_truncation_at_max_list_boundary() {
    let mut ipc_src = String::new();
    for i in 0..5_000 {
        let _ = writeln!(ipc_src, "ipcRenderer.invoke(\"ch_{i}\");");
    }
    let ipc_exact = analyze("ipc_exact.js", &ipc_src, &lexer_limits()).facts;
    assert_eq!(ipc_exact.ipc.len(), 5_000);
    assert!(!ipc_exact.truncated.contains(&"ipc".to_string()));

    ipc_src.push_str("ipcRenderer.invoke(\"ch_5000\");\n");
    let ipc_over = analyze("ipc_over.js", &ipc_src, &lexer_limits()).facts;
    assert_eq!(ipc_over.ipc.len(), 5_000);
    assert!(ipc_over.truncated.contains(&"ipc".to_string()));

    let mut exp_src = String::new();
    for i in 0..5_000 {
        let _ = writeln!(exp_src, "contextBridge.exposeInMainWorld(\"api_{i}\", obj);");
    }
    let exp_exact = analyze("exp_exact.js", &exp_src, &lexer_limits()).facts;
    assert_eq!(exp_exact.exposed.len(), 5_000);
    assert!(!exp_exact.truncated.contains(&"exposed".to_string()));

    exp_src.push_str("contextBridge.exposeInMainWorld(\"api_5000\", obj);\n");
    let exp_over = analyze("exp_over.js", &exp_src, &lexer_limits()).facts;
    assert_eq!(exp_over.exposed.len(), 5_000);
    assert!(exp_over.truncated.contains(&"exposed".to_string()));

    let mut url_src = String::new();
    for i in 0..5_000 {
        let _ = writeln!(url_src, "const u{i} = \"https://ex.test/p/{i}\";");
    }
    url_src.push_str("const dup = \"https://ex.test/p/0\";\n");
    let url_exact = analyze("url_exact.js", &url_src, &lexer_limits()).facts;
    assert_eq!(url_exact.endpoints.len(), 5_000);
    assert!(!url_exact.truncated.contains(&"endpoints".to_string()), "duplicate URL at cap must not mark endpoints truncated: {:?}", url_exact.truncated);

    url_src.push_str("const u5000 = \"https://ex.test/p/5000\";\n");
    let url_over = analyze("url_over.js", &url_src, &lexer_limits()).facts;
    assert_eq!(url_over.endpoints.len(), 5_000);
    assert!(url_over.truncated.contains(&"endpoints".to_string()));
}

/// Ordinary imports, IPC channels, and bundler markers in lexer fallback retain existing behavior.
#[test]
fn ordinary_lexer_fallback_imports_ipc_and_markers_control() {
    let src = r#"
        import "polyfill.js";
        const electron = require("electron");
        ipcMain.handle("notes:list", fn);
        __webpack_require__(0);
    "#;
    let facts = analyze("bundle.js", src, &lexer_limits()).facts;
    assert_eq!(facts.mode, ParseMode::Lexer);
    assert_eq!(facts.imports.len(), 2);
    assert_eq!(facts.ipc.len(), 1);
    assert_eq!(facts.ipc[0].side, "main");
    assert_eq!(facts.bundler_markers.get("__webpack_require__"), Some(&1));
    assert!(facts.truncated.is_empty());
}
