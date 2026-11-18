//! Regression tests for exclusive, priority-ordered module matching in `js_compare`.

use craft_anything_analysis::js_app::{AppGraph, BundleModuleSummary, ModuleSummary};
use craft_anything_analysis::js_compare::{compare_export_outlines, compare_versions};
use craft_anything_js::ExportShape;

/// Build a minimal script `ModuleSummary` for comparison tests.
fn script(path: &str, sha256: &str, fingerprint: Option<&str>, exports: &[&str]) -> ModuleSummary {
    ModuleSummary {
        path: path.into(),
        kind: "script".into(),
        sha256: sha256.into(),
        fingerprint: fingerprint.map(String::from),
        exports: ExportShape { esm: exports.iter().map(|s| (*s).to_string()).collect(), ..Default::default() },
        ..Default::default()
    }
}

/// `compare_export_outlines` must claim each right-hand script at most once and prefer exact path
/// matches over structural fingerprint fallbacks regardless of module order.
#[test]
fn export_outlines_keep_exact_paths_and_single_claims() {
    let left =
        AppGraph { modules: vec![script("b.js", "s_b", Some("fp_shared"), &["b"]), script("a.js", "s_a", Some("fp_shared"), &["a"])], ..Default::default() };
    let right = AppGraph { modules: vec![script("a.js", "s_a", Some("fp_shared"), &["a"])], ..Default::default() };
    let d = compare_export_outlines(&left, &right);
    let rows = d["modules"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "{d}");
    let row_a = rows.iter().find(|r| r["left"] == "a.js").unwrap();
    let row_b = rows.iter().find(|r| r["left"] == "b.js").unwrap();
    assert_eq!(row_a["right"], "a.js", "exact path match must survive earlier fallback candidates: {d}");
    assert_eq!(row_a["same"], true);
    assert_eq!(row_b["right"], serde_json::Value::Null, "right module must not be claimed twice: {d}");
    assert_eq!(row_b["removed"], true);
}

/// `compare_export_outlines` only pairs `script` modules, never non-script modules.
#[test]
fn export_outlines_ignore_non_script_right_modules() {
    let left = AppGraph { modules: vec![script("page.js", "s1", Some("fp_same"), &["x"])], ..Default::default() };
    let mut non_script = script("page.html", "s1", Some("fp_same"), &[]);
    non_script.kind = "html".into();
    let right = AppGraph { modules: vec![non_script], ..Default::default() };
    let d = compare_export_outlines(&left, &right);
    let rows = d["modules"].as_array().unwrap();
    assert_eq!(rows[0]["left"], "page.js");
    assert_eq!(rows[0]["removed"], true, "{d}");
}

/// `compare_versions` must not let an earlier content-identical removed copy steal a later
/// exact-path unchanged module or double-count the right-hand module.
#[test]
fn version_comparison_prefers_exact_path_over_moved_content() {
    let left =
        AppGraph { modules: vec![script("old_copy.js", "sha_same", Some("fp1"), &[]), script("keep.js", "sha_same", Some("fp1"), &[])], ..Default::default() };
    let right = AppGraph { modules: vec![script("keep.js", "sha_same", Some("fp1"), &[])], ..Default::default() };
    let d = compare_versions(&left, &right);
    let mods = &d["modules"];
    assert_eq!(mods["unchanged"], 1, "{mods}");
    assert!(mods["moved"].as_array().unwrap().is_empty(), "keep.js cannot be both unchanged and moved: {mods}");
    assert_eq!(mods["removed"], serde_json::json!(["old_copy.js"]), "{mods}");
    assert!(mods["added"].as_array().unwrap().is_empty(), "{mods}");
}

/// `compare_versions` must let a unique build-hashless name match beat an earlier fingerprint match.
#[test]
fn version_comparison_prefers_hashless_name_over_fingerprint_rename() {
    let left = AppGraph {
        modules: vec![script("removed.js", "sha_rem", Some("fp_shared"), &[]), script("assets/Doctor-DIpf_I7N.js", "sha_doc_l", Some("fp_shared"), &[])],
        ..Default::default()
    };
    let right = AppGraph { modules: vec![script("assets/Doctor-DrkWsav4.js", "sha_doc_r", Some("fp_shared"), &[])], ..Default::default() };
    let d = compare_versions(&left, &right);
    let mods = &d["modules"];
    let changed = mods["changed"].as_array().unwrap();
    assert_eq!(changed.len(), 1, "{mods}");
    assert_eq!(changed[0]["was"], "assets/Doctor-DIpf_I7N.js");
    assert_eq!(changed[0]["path"], "assets/Doctor-DrkWsav4.js");
    assert!(mods["renamed"].as_array().unwrap().is_empty(), "{mods}");
    assert_eq!(mods["removed"], serde_json::json!(["removed.js"]), "{mods}");
}

/// Bundle module matching must prefer content and structure tiers across the whole module list
/// before falling back to id matching.
#[test]
fn bundle_module_comparison_prefers_higher_tiers_before_id_fallback() {
    let bm = |id: &str, sha: &str, fp: &str| BundleModuleSummary {
        id: id.into(),
        format: "webpack".into(),
        sha256: sha.into(),
        fingerprint: fp.into(),
        ..Default::default()
    };
    let left = AppGraph {
        modules: vec![ModuleSummary {
            path: "dist/app.js".into(),
            kind: "script".into(),
            bundle_modules: vec![bm("1", "sha_old_1", "fp_1"), bm("2", "sha_2", "fp_2")],
            ..Default::default()
        }],
        ..Default::default()
    };
    let right = AppGraph {
        modules: vec![ModuleSummary {
            path: "dist/app.js".into(),
            kind: "script".into(),
            bundle_modules: vec![bm("1", "sha_new", "fp_2")],
            ..Default::default()
        }],
        ..Default::default()
    };
    let d = compare_versions(&left, &right);
    let bms = &d["bundle_modules"];
    assert_eq!(bms["matched_by"]["structure"], 1, "{bms}");
    assert_eq!(bms["changed_total"], 0, "id fallback must not steal structure match: {bms}");
    assert_eq!(bms["removed"].as_array().unwrap().len(), 1, "{bms}");
    assert_eq!(bms["removed"][0]["id"], "1", "{bms}");
}
