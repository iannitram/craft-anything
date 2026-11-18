//! Regression tests for `js_compare::compare_versions` preserving non-empty and distinct
//! vendor versions instead of letting trailing markers or later entries overwrite them.

use craft_anything_analysis::js_app::{AppFile, AppGraph, AppInput, AppOptions, analyze_application};
use craft_anything_analysis::js_compare::compare_versions;
use serde_json::json;

/// Trailing empty, null, or whitespace-only vendor versions must not overwrite earlier known versions.
#[test]
fn trailing_empty_marker_does_not_erase_banner_or_package_version() {
    let left = AppGraph {
        graph_id: "left".into(),
        vendors: vec![
            json!({"name": "react-dom", "version": "18.2.0", "source": "banner"}),
            json!({"name": "react-dom", "version": null, "source": "marker"}),
            json!({"name": "lodash", "version": "4.17.20", "source": "package-json"}),
            json!({"name": "lodash", "version": "  ", "source": "marker"}),
        ],
        ..Default::default()
    };
    let right = AppGraph {
        graph_id: "right".into(),
        vendors: vec![
            json!({"name": "react-dom", "version": "18.3.0", "source": "banner"}),
            json!({"name": "react-dom", "version": "", "source": "marker"}),
            json!({"name": "lodash", "version": null, "source": "marker"}),
            json!({"name": "lodash", "version": "4.17.21", "source": "package-json"}),
        ],
        ..Default::default()
    };

    let diff = compare_versions(&left, &right);
    assert_eq!(
        diff["vendors"],
        json!([
            {"name": "lodash", "from": "4.17.20", "to": "4.17.21"},
            {"name": "react-dom", "from": "18.2.0", "to": "18.3.0"},
        ])
    );
    assert_eq!(diff["vendors_total"], 2);
}

/// Real application analysis of multi-chunk bundles with a banner in one file and a marker in another keeps the version diff.
#[test]
fn end_to_end_analyze_application_keeps_banner_version_over_marker() {
    let analyze = |version: &str| {
        let banner = format!("/*! react-dom v{version} | MIT */\nexport const v = 1;\n");
        let chunk = "export const fiber = \"__reactFiber$\";\n".to_string();
        let input = AppInput {
            files: vec![
                AppFile { path: "renderer/a-vendor.js".into(), bytes: banner.into_bytes() },
                AppFile { path: "renderer/b-chunk.js".into(), bytes: chunk.into_bytes() },
            ],
            listing: vec![("renderer/a-vendor.js".into(), 64), ("renderer/b-chunk.js".into(), 64)],
            ..Default::default()
        };
        analyze_application(&input, &AppOptions::default(), &|| false).unwrap()
    };

    let left = analyze("18.2.0");
    let right = analyze("18.3.0");
    assert!(left.vendors.iter().any(|v| v["name"] == "react-dom" && v["source"] == "banner" && v["version"] == "18.2.0"));
    assert!(left.vendors.iter().any(|v| v["name"] == "react-dom" && v["source"] == "marker" && v["version"].is_null()));

    let diff = compare_versions(&left, &right);
    assert_eq!(diff["vendors"], json!([{"name": "react-dom", "from": "18.2.0", "to": "18.3.0"}]));
}

/// Multiple distinct versions observed for the same vendor stay sorted and deduplicated regardless of input order.
#[test]
fn multiple_distinct_versions_are_retained_and_sorted_instead_of_overwritten() {
    let left_asc = AppGraph {
        graph_id: "left-asc".into(),
        vendors: vec![
            json!({"name": "semver", "version": "7.5.4"}),
            json!({"name": "semver", "version": "7.6.0"}),
            json!({"name": "semver", "version": "7.5.4"}),
            json!({"name": "axios", "version": "1.6.0"}),
            json!({"name": "axios", "version": "1.6.0"}),
        ],
        ..Default::default()
    };
    let left_desc = AppGraph { graph_id: "left-desc".into(), vendors: left_asc.vendors.iter().rev().cloned().collect(), ..Default::default() };
    let right = AppGraph {
        graph_id: "right".into(),
        vendors: vec![json!({"name": "semver", "version": "7.6.0"}), json!({"name": "axios", "version": "1.6.0"})],
        ..Default::default()
    };

    for left in [left_asc, left_desc] {
        let diff = compare_versions(&left, &right);
        assert_eq!(diff["vendors"], json!([{"name": "semver", "from": "7.5.4, 7.6.0", "to": "7.6.0"}]));
    }
}

/// Unversioned-only matches stay equal and added/removed vendors preserve their existing schema.
#[test]
fn unversioned_and_added_removed_vendors_keep_existing_behavior() {
    let left = AppGraph {
        graph_id: "left".into(),
        vendors: vec![json!({"name": "sentry", "version": null}), json!({"name": "removed-lib", "version": "1.0.0"})],
        ..Default::default()
    };
    let right = AppGraph {
        graph_id: "right".into(),
        vendors: vec![json!({"name": "sentry", "version": ""}), json!({"name": "added-lib", "version": "2.0.0"})],
        ..Default::default()
    };

    let diff = compare_versions(&left, &right);
    assert_eq!(
        diff["vendors"],
        json!([
            {"name": "added-lib", "added": true},
            {"name": "removed-lib", "removed": true},
        ])
    );
}
