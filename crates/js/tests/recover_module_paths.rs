//! Regression tests for `recover::safe_module_path` single-scheme stripping and
//! extension-preserving collision deduplication in `recover::recover_modules`.

use craft_anything_js::BundleModule;
use craft_anything_js::recover::{RewriteLevel, recover_modules, safe_module_path};
use std::collections::HashSet;

fn synthetic_module(id: &str, hash: &str, len: u32) -> BundleModule {
    BundleModule {
        id: id.into(),
        format: "webpack".into(),
        chunk_ids: vec![],
        start: 0,
        end: len,
        body: Some((0, len)),
        line: 1,
        params: vec![],
        deps: vec![],
        sha256: hash.into(),
        fingerprint: String::new(),
    }
}

/// `safe_module_path` must strip at most one scheme prefix without eating chained prefixes or leading name chars.
#[test]
fn safe_module_path_strips_only_one_scheme_prefix() {
    assert_eq!(safe_module_path("webpack://pack/index.js", ".js"), "pack/index.js");
    assert_eq!(safe_module_path("webpack://web/app.js", ".js"), "web/app.js");
    assert_eq!(safe_module_path("webpack:./pkg/entry.js", ".js"), "pkg/entry.js");
    assert_eq!(safe_module_path("file://lib/helpers.js", ".js"), "lib/helpers.js");
    assert_eq!(safe_module_path("webpack://webpack:entry.js", ".js"), "webpack_entry.js");
    assert_eq!(safe_module_path("webpack://webpack:pkg/entry.js", ".js"), "webpack_pkg/entry.js");
    assert_eq!(safe_module_path("webpack://webpack://src/a.js", ".js"), "webpack_/src/a.js");
    assert_eq!(safe_module_path("webpack://file://src/a.js", ".js"), "file_/src/a.js");
}

/// Existing safe-path normalization rules (`webpack:///`, Windows separators, traversal segments, leading dots) remain intact.
#[test]
fn safe_module_path_keeps_ordinary_normalization_controls() {
    assert_eq!(safe_module_path("webpack:///./src/a.ts", ".js"), "src/a.ts");
    assert_eq!(safe_module_path("./src/../../etc/passwd", ".js"), "src/etc/passwd.js");
    assert_eq!(safe_module_path("C:\\x\\y", ".js"), "x/y.js");
    assert_eq!(safe_module_path(".hidden", ".js"), "hidden.js");
}

/// Collision deduplication in `recover_modules` must preserve `.js.js` stems and non-`.js` extensions.
#[test]
fn duplicate_non_js_and_double_js_modules_preserve_stem_and_extension() {
    let source = "globalThis.x = 1;";
    let len = source.len() as u32;
    let modules = vec![
        synthetic_module("src/app.ts", "0123456789abcdef", len),
        synthetic_module("src/app.ts", "fedcba9876543210", len),
        synthetic_module("styles.css", "aaaaaaaa11111111", len),
        synthetic_module("styles.css", "bbbbbbbb22222222", len),
        synthetic_module("mod.mjs", "cccccccc33333333", len),
        synthetic_module("mod.mjs", "dddddddd44444444", len),
        synthetic_module("foo.js.js", "eeeeeeee55555555", len),
        synthetic_module("foo.js.js", "ffffffff66666666", len),
    ];

    let recovered = recover_modules(source, &modules, RewriteLevel::None, &craft_anything_js::Limits::default());
    let paths: Vec<&str> = recovered.iter().map(|m| m.file_name.as_str()).collect();
    assert_eq!(
        paths,
        ["src/app.ts", "src/app.fedcba98.ts", "styles.css", "styles.bbbbbbbb.css", "mod.mjs", "mod.dddddddd.mjs", "foo.js.js", "foo.js.ffffffff.js",]
    );
}

/// Repeated hash collisions and pre-hashed names on non-`.js` extensions must stay unique and keep their extension.
#[test]
fn repeated_collisions_on_non_js_extensions_remain_distinct() {
    let source = "globalThis.y = 2;";
    let len = source.len() as u32;
    let modules = vec![
        synthetic_module("view.tsx", "0123456789abcdef", len),
        synthetic_module("view.tsx", "0123456789abcdef", len),
        synthetic_module("view.tsx", "0123456789abcdef", len),
        synthetic_module("view.01234567.tsx", "0123456789abcdef", len),
    ];

    let recovered = recover_modules(source, &modules, RewriteLevel::None, &craft_anything_js::Limits::default());
    let unique: HashSet<&str> = recovered.iter().map(|m| m.file_name.as_str()).collect();
    assert_eq!(unique.len(), recovered.len());
    assert_eq!(recovered[0].file_name, "view.tsx");
    assert_eq!(recovered[1].file_name, "view.01234567.tsx");
    assert_eq!(recovered[2].file_name, "view.01234567.2.tsx");
    assert_eq!(recovered[3].file_name, "view.01234567.01234567.tsx");
}
