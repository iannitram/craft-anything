//! Regression tests for `js_app` source-map `sources_with_content` accumulation across
//! indexed `sections` and exact `.map` suffix stripping on stand-alone map entries.

use craft_anything_analysis::js_app::{AppFile, AppGraph, AppInput, AppOptions, analyze_application};
use serde_json::{Value, json};

fn analyze_files(files: Vec<(&str, Vec<u8>)>, extra_listing: &[(&str, u64)]) -> AppGraph {
    let mut listing: Vec<(String, u64)> = files.iter().map(|(path, bytes)| ((*path).to_string(), bytes.len() as u64)).collect();
    for (path, size) in extra_listing {
        listing.push(((*path).to_string(), *size));
    }
    let app_files = files.into_iter().map(|(path, bytes)| AppFile { path: path.to_string(), bytes }).collect();
    analyze_application(&AppInput { files: app_files, listing, ..Default::default() }, &AppOptions::default(), &|| false).unwrap()
}

fn json_bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).unwrap()
}

/// Indexed source maps (`sections`) must sum non-null `sourcesContent` entries across every inline section.
#[test]
fn indexed_source_map_counts_content_across_inline_sections() {
    let map = json!({
        "version": 3,
        "file": "dist/app.js",
        "sections": [
            {
                "offset": { "line": 0, "column": 0 },
                "map": {
                    "version": 3,
                    "sources": ["src/a.js", "src/b.js", "src/c.js"],
                    "sourcesContent": ["export const a = 1;", null, "export const c = 3;"],
                    "mappings": ""
                }
            },
            {
                "offset": { "line": 10, "column": 0 },
                "map": {
                    "version": 3,
                    "sources": ["src/d.js"],
                    "sourcesContent": ["export const d = 4;"],
                    "mappings": ""
                }
            }
        ]
    });
    let graph = analyze_files(vec![("dist/app.js", b"//# sourceMappingURL=app.js.map\n".to_vec()), ("dist/app.js.map", json_bytes(&map))], &[]);
    assert_eq!(graph.source_maps.len(), 1);
    let entry = &graph.source_maps[0];
    assert_eq!(entry.sources, ["src/a.js", "src/b.js", "src/c.js", "src/d.js"]);
    assert_eq!(entry.sources_with_content, 3);
    assert!(!entry.unreadable);
}

/// Nested inline `sections` and external `url` sections must accumulate embedded content from every nested leaf.
#[test]
fn nested_indexed_source_maps_and_external_sections_accumulate_embedded_content() {
    let map = json!({
        "version": 3,
        "file": "dist/nested.js",
        "sections": [
            {
                "offset": { "line": 0, "column": 0 },
                "map": {
                    "version": 3,
                    "sections": [
                        {
                            "offset": { "line": 0, "column": 0 },
                            "map": {
                                "version": 3,
                                "sources": ["inner/one.ts", "inner/two.ts"],
                                "sourcesContent": ["export const one = 1;", "export const two = 2;"],
                                "mappings": ""
                            }
                        },
                        {
                            "offset": { "line": 5, "column": 0 },
                            "url": "external-chunk.js.map"
                        }
                    ]
                }
            },
            {
                "offset": { "line": 20, "column": 0 },
                "map": {
                    "version": 3,
                    "sources": ["outer/three.ts"],
                    "sourcesContent": ["export const three = 3;"],
                    "mappings": ""
                }
            }
        ]
    });
    let graph = analyze_files(
        vec![("dist/nested.js", b"// standalone map test without sourceMappingURL comment\n".to_vec()), ("dist/nested.js.map", json_bytes(&map))],
        &[],
    );
    let entry = graph.source_maps.iter().find(|s| s.resolved.as_deref() == Some("dist/nested.js.map")).unwrap();
    assert_eq!(entry.sources, ["inner/one.ts", "inner/two.ts", "outer/three.ts"]);
    assert_eq!(entry.external_sections, 1);
    assert_eq!(entry.sources_with_content, 3);
}

/// Stand-alone `.map` inventory entries must strip only a single trailing `.map` suffix, never preceding `.`, `m`, `a`, `p` characters.
#[test]
fn stand_alone_map_suffix_strips_only_the_literal_map_extension() {
    let flat_map = json!({
        "version": 3,
        "sources": ["src/index.js"],
        "sourcesContent": ["console.log(1);"],
        "mappings": ""
    });
    let graph = analyze_files(
        vec![("dist/app.js", b"console.log(1);\n".to_vec()), ("dist/app.js.map", json_bytes(&flat_map))],
        &[("dist/app.amp.map", 128), ("data/bitmap.map", 256), ("dist/app.map.map", 64)],
    );

    let find_script = |resolved: &str| graph.source_maps.iter().find(|s| s.resolved.as_deref() == Some(resolved)).map(|s| s.script.as_str());
    assert_eq!(find_script("dist/app.js.map"), Some("dist/app.js"));
    assert_eq!(find_script("dist/app.amp.map"), Some("dist/app.amp"));
    assert_eq!(find_script("data/bitmap.map"), Some("data/bitmap"));
    assert_eq!(find_script("dist/app.map.map"), Some("dist/app.map"));
    let loaded = graph.source_maps.iter().find(|s| s.resolved.as_deref() == Some("dist/app.js.map")).unwrap();
    assert_eq!(loaded.sources_with_content, 1);
}

/// Flat maps and indexed maps whose `mappings` fail decoding (`parse_sources` fallback) both retain content counts.
#[test]
fn flat_source_map_and_bad_mappings_fallback_retain_content_counts() {
    let indexed_bad_mappings = json!({
        "version": 3,
        "sections": [
            {
                "offset": { "line": 0, "column": 0 },
                "map": {
                    "version": 3,
                    "sources": ["src/ok.js", "src/no_content.js"],
                    "sourcesContent": ["const ok = true;", null],
                    "mappings": ";;INVALID_VLQ_!!!"
                }
            }
        ]
    });
    let flat = json!({
        "version": 3,
        "sources": ["src/plain.js"],
        "sourcesContent": ["export default 1;"],
        "mappings": ""
    });
    let graph = analyze_files(
        vec![
            ("bad.js", b"//# sourceMappingURL=bad.js.map\n".to_vec()),
            ("bad.js.map", json_bytes(&indexed_bad_mappings)),
            ("plain.js", b"//# sourceMappingURL=plain.js.map\n".to_vec()),
            ("plain.js.map", json_bytes(&flat)),
        ],
        &[],
    );
    let bad = graph.source_maps.iter().find(|s| s.script == "bad.js").unwrap();
    assert_eq!(bad.sources, ["src/ok.js", "src/no_content.js"]);
    assert_eq!(bad.sources_with_content, 1);
    let plain = graph.source_maps.iter().find(|s| s.script == "plain.js").unwrap();
    assert_eq!(plain.sources_with_content, 1);
}
