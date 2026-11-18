//! Regression tests for `web_scripts::trace_location` on indexed Source Map v3 bundles.

use craft_anything_analysis::web_scripts::{parse_map, trace_location};
use serde_json::json;

/// Flat (non-indexed) source maps continue to return source snippets, ignore status, and source counts.
#[test]
fn flat_source_map_trace_returns_context_and_ignore_flag() {
    let raw = serde_json::to_vec(&json!({
        "version": 3,
        "sources": ["src/app.ts"],
        "sourcesContent": ["const a = 1;\nconst b = 2;\nexport { a, b };\n"],
        "ignoreList": [0],
        "names": ["b"],
        "mappings": ";AACA"
    }))
    .unwrap();
    let map = parse_map(&raw).unwrap();
    let traced = trace_location(&map, 2, 0, 1);
    assert_eq!(traced["found"], true);
    assert_eq!(traced["sources"], 1);
    assert_eq!(traced["original_positions"][0]["ignored"], true);
    assert_eq!(traced["original_positions"][0]["context"][1]["text"], "const b = 2;");
}

/// Indexed source maps (`sections`) must surface each section's `sourcesContent`, `ignoreList`, and total `sources`.
#[test]
fn indexed_source_map_trace_reads_active_section_content_and_ignore_list() {
    let raw = serde_json::to_vec(&json!({
        "version": 3,
        "file": "bundle.js",
        "sections": [
            {
                "offset": { "line": 0, "column": 0 },
                "map": {
                    "version": 3,
                    "sourceRoot": "webpack:///vendor/",
                    "sources": ["polyfill.js"],
                    "sourcesContent": ["// polyfill line 1\nwindow.__poly = true;\n// polyfill line 3\n"],
                    "ignoreList": [0],
                    "mappings": "AACA"
                }
            },
            {
                "offset": { "line": 2, "column": 4 },
                "map": {
                    "version": 3,
                    "sourceRoot": "webpack:///app/",
                    "sources": ["helper.ts", "main.ts"],
                    "sourcesContent": [
                        "export const k = 0;\n",
                        "import { k } from './helper';\nexport function run() {\n  return k + 1;\n}\n"
                    ],
                    "ignoreList": [0],
                    "mappings": "ECEA"
                }
            }
        ]
    }))
    .unwrap();

    let map = parse_map(&raw).unwrap();

    // First section (generated line 1, col 0 -> polyfill.js line 2, ignored = true).
    let first = trace_location(&map, 1, 0, 1);
    assert_eq!(first["found"], true);
    assert_eq!(first["sources"], 3, "indexed map should report total sources across sections: {first}");
    let pos0 = &first["original_positions"][0];
    assert_eq!(pos0["source"], "webpack:///vendor/polyfill.js");
    assert_eq!(pos0["line"], 2);
    assert_eq!(pos0["ignored"], true, "polyfill.js is in section 0 ignoreList: {pos0}");
    let ctx0 = pos0["context"].as_array().expect("section 0 sourcesContent should populate context");
    assert_eq!(ctx0.len(), 3);
    assert_eq!(ctx0[1], json!({"line": 2, "text": "window.__poly = true;"}));

    // Second section at column-shifted offset (generated line 3, col 6 -> main.ts (source_index 1, NOT ignored) line 3).
    let second = trace_location(&map, 3, 6, 1);
    assert_eq!(second["found"], true);
    assert_eq!(second["sources"], 3);
    let pos1 = &second["original_positions"][0];
    assert_eq!(pos1["source"], "webpack:///app/main.ts");
    assert_eq!(pos1["line"], 3);
    assert_eq!(pos1["ignored"], false, "source_index 1 is not in section 1 ignoreList: {pos1}");
    let ctx1 = pos1["context"].as_array().expect("section 1 sourcesContent should populate context");
    assert_eq!(ctx1[1], json!({"line": 3, "text": "  return k + 1;"}));
}
