//! A source-to-bundle comparison that examines only `max_rows` scripts says how many it examined,
//! and the unclaimed-unit count stays complete even when only some rows are listed.

use craft_anything_analysis::js_app::{AppFile, AppGraph, AppInput, AppOptions, analyze_application};
use craft_anything_analysis::js_source::{SourceGraph, build_source_graph, diff_source_and_bundle};
use serde_json::json;

fn app_graph(files: &[(&str, &str)]) -> AppGraph {
    let files: Vec<AppFile> = files.iter().map(|(path, src)| AppFile { path: (*path).into(), bytes: src.as_bytes().to_vec() }).collect();
    let listing = files.iter().map(|f| (f.path.clone(), f.bytes.len() as u64)).collect();
    analyze_application(&AppInput { files, listing, ..Default::default() }, &AppOptions::default(), &|| false).unwrap()
}

fn source_graph(paths: &[&str]) -> SourceGraph {
    let files: Vec<_> = paths.iter().map(|path| ((*path).to_string(), Vec::new())).collect();
    build_source_graph("reference", &files, 0, false, &Default::default(), &|| false).unwrap()
}

/// A capped comparison examines only the first script but reports that beside the total, and the
/// unclaimed-unit count stays complete even though only one row is listed.
#[test]
fn examined_scripts_and_unclaimed_units_are_counted_in_full() {
    let graph = app_graph(&[("a.js", "const a = 1;\n"), ("b.js", "const b = 2;\n"), ("c.js", "const c = 3;\n")]);
    let src = source_graph(&["a.js", "b.js"]);
    let diff = diff_source_and_bundle(&src, &graph, 1);
    let summary = &diff["summary"];
    assert_eq!(summary["scripts_examined"], json!(1), "{diff}");
    assert_eq!(summary["source_scripts"], json!(2), "{diff}");
    assert_eq!(summary["matched"], json!(1), "{diff}");
    assert_eq!(summary["unmatched_sources"], json!(0), "{diff}");
    assert_eq!(summary["uncertain_sources"], json!(0), "{diff}");
    assert_eq!(diff["rows_capped"], json!(true), "{diff}");
    // b.js and c.js are unclaimed: the count is complete even though only one row is listed.
    assert_eq!(summary["bundle_units_without_source"], json!(2), "{diff}");
    assert_eq!(diff["bundle_units_without_source"].as_array().map(Vec::len), Some(1), "{diff}");
}

/// With room for every script, the comparison examines all of them and does not claim a cap.
#[test]
fn an_uncapped_comparison_reports_every_script() {
    let graph = app_graph(&[("a.js", "const a = 1;\n"), ("b.js", "const b = 2;\n")]);
    let src = source_graph(&["a.js", "b.js"]);
    let diff = diff_source_and_bundle(&src, &graph, 10);
    let summary = &diff["summary"];
    assert_eq!(summary["scripts_examined"], json!(2), "{diff}");
    assert_eq!(summary["source_scripts"], json!(2), "{diff}");
    assert_eq!(summary["matched"], json!(2), "{diff}");
    assert_eq!(summary["bundle_units_without_source"], json!(0), "{diff}");
    assert_eq!(diff["rows_capped"], json!(false), "{diff}");
}
