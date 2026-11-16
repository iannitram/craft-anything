use craft_anything_analysis::js_app::{AppFile, AppGraph, AppInput, AppOptions, analyze_application};
use craft_anything_analysis::js_source::{SourceGraph, build_source_graph, diff_source_and_bundle};
use serde_json::{Value, json};

fn graph_with_maps(maps: Vec<Value>) -> AppGraph {
    let mut files = Vec::new();
    for (i, map) in maps.into_iter().enumerate() {
        files.push(AppFile { path: format!("script-{i}.js"), bytes: format!("//# sourceMappingURL=script-{i}.js.map\n").into_bytes() });
        files.push(AppFile { path: format!("script-{i}.js.map"), bytes: serde_json::to_vec(&map).unwrap() });
    }
    let listing = files.iter().map(|file| (file.path.clone(), file.bytes.len() as u64)).collect();
    analyze_application(&AppInput { files, listing, ..Default::default() }, &AppOptions::default(), &|| false).unwrap()
}

fn source_graph(paths: &[&str]) -> SourceGraph {
    let files: Vec<_> = paths.iter().map(|path| ((*path).to_string(), Vec::new())).collect();
    build_source_graph("reference", &files, 0, false, &Default::default(), &|| false).unwrap()
}

#[test]
fn application_inventory_reports_the_entry_limit() {
    let sources: Vec<_> = (0..5001).map(|i| format!("source-{i}.js")).collect();
    let graph = graph_with_maps(vec![json!({"version": 3, "sourceRoot": "src", "sources": sources, "mappings": ""})]);
    let map = &graph.source_maps[0];
    assert_eq!(map.sources.len(), 5000);
    assert_eq!(map.sources.first().map(String::as_str), Some("src/source-0.js"));
    assert_eq!(map.sources.last().map(String::as_str), Some("src/source-4999.js"));
    assert!(map.sources_truncated);
    assert!(graph.limitations.iter().any(|(key, message)| key == "source_map_inventory_truncated" && message.contains("script-0.js")));
}

#[test]
fn expanded_path_bytes_are_bounded_across_application_maps() {
    let maps = ["first", "second"]
        .into_iter()
        .map(|prefix| {
            json!({
                "version": 3, "sourceRoot": "r".repeat(2 * 1024 * 1024),
                "sources": (0..20).map(|i| format!("/{prefix}-{i}.js")).collect::<Vec<_>>(), "mappings": ""
            })
        })
        .collect();
    let graph = graph_with_maps(maps);
    assert_eq!(graph.source_maps.len(), 2);
    assert_eq!(graph.source_maps[0].sources.len(), 20);
    assert!(!graph.source_maps[0].sources_truncated);
    assert_eq!(graph.source_maps[1].sources.len(), 11);
    assert!(graph.source_maps[1].sources_truncated);
    let bytes: usize = graph.source_maps.iter().flat_map(|map| &map.sources).map(String::len).sum();
    assert!(bytes <= 64 * 1024 * 1024);
    assert!(graph.limitations.iter().any(|(key, message)| key == "source_map_inventory_truncated" && message.contains("script-1.js")));

    let sources = source_graph(&["first-0.js", "second-0.js", "second-19.js", "not-listed.js", "script-1.js"]);
    let diff = diff_source_and_bundle(&sources, &graph, 100);
    assert_eq!(diff["unmatched_sources"], json!([]), "omitted sources are not confirmed absent: {diff}");
    assert_eq!(
        diff["uncertain_sources"],
        json!([
            {"source": "second-19.js", "reason": "source_map_inventory_truncated"},
            {"source": "not-listed.js", "reason": "source_map_inventory_truncated"}
        ])
    );
    assert_eq!(diff["summary"]["uncertain_sources"], 2);
    assert_eq!(diff["summary"]["unmatched_sources"], 0);
    assert_eq!(diff["summary"]["matched"], 3);
    assert_eq!(diff["matches"][0]["unit"], "script-0.js");
    assert_eq!(diff["matches"][1]["unit"], "script-1.js");
    assert_eq!(diff["matches"][2]["quality"], "strong");
    assert_eq!(diff["source_map_inventory_truncated"], true);

    let first_row = diff_source_and_bundle(&sources, &graph, 1);
    assert_eq!(first_row["summary"]["matched"], 1);
    assert_eq!(first_row["uncertain_sources"], json!([]));
    assert_eq!(first_row["source_map_inventory_truncated"], true, "positive matches do not make the inventory complete");
}

#[test]
fn ordinary_inventories_keep_paths_content_counts_and_complete_evidence() {
    let graph = graph_with_maps(vec![json!({
        "version": 3, "sourceRoot": "src", "sources": ["a.js", null, "/b.js"],
        "sourcesContent": ["first();", null, "second();"], "mappings": ""
    })]);
    let map = &graph.source_maps[0];
    assert_eq!(map.sources, ["src/a.js", "src//b.js"]);
    assert_eq!(map.sources_with_content, 2);
    assert!(!map.sources_truncated);
    assert!(!graph.limitations.iter().any(|(key, _)| key == "source_map_inventory_truncated"));
    let mut saved = serde_json::to_value(&graph).unwrap();
    saved["source_maps"][0].as_object_mut().unwrap().remove("sources_truncated");
    let restored: AppGraph = serde_json::from_value(saved).unwrap();
    assert!(!restored.source_maps[0].sources_truncated);
    assert_eq!(restored.source_maps[0].sources, map.sources);

    for app in [graph, restored] {
        let diff = diff_source_and_bundle(&source_graph(&["src/a.js", "src/missing.js"]), &app, 100);
        assert_eq!(diff["summary"]["matched"], 1);
        assert_eq!(diff["unmatched_sources"], json!(["src/missing.js"]));
        assert_eq!(diff["summary"]["unmatched_sources"], 1);
        assert_eq!(diff["uncertain_sources"], json!([]));
        assert_eq!(diff["summary"]["uncertain_sources"], 0);
        assert_eq!(diff["source_map_inventory_truncated"], false);
    }
}

#[test]
fn missing_sources_without_maps_remain_unmatched() {
    let diff = diff_source_and_bundle(&source_graph(&["src/missing.js"]), &graph_with_maps(Vec::new()), 100);
    assert_eq!(diff["unmatched_sources"], json!(["src/missing.js"]));
    assert_eq!(diff["summary"]["unmatched_sources"], 1);
    assert_eq!(diff["uncertain_sources"], json!([]));
    assert_eq!(diff["summary"]["uncertain_sources"], 0);
    assert_eq!(diff["source_map_inventory_truncated"], false);
}

#[test]
fn a_map_refused_for_too_many_entries_leaves_its_sources_uncertain() {
    // More names than the parser keeps by default: the map is refused, not read as empty.
    let names = vec!["\"\""; craft_anything_formats::sourcemap::Limits::default().max_entries + 1].join(",");
    let map = format!("{{\"version\":3,\"sources\":[\"a.js\"],\"mappings\":\"\",\"names\":[{names}]}}").into_bytes();
    let files = vec![
        AppFile { path: "script-0.js".into(), bytes: b"//# sourceMappingURL=script-0.js.map\n".to_vec() },
        AppFile { path: "script-0.js.map".into(), bytes: map },
    ];
    let listing = files.iter().map(|file| (file.path.clone(), file.bytes.len() as u64)).collect();
    let graph = analyze_application(&AppInput { files, listing, ..Default::default() }, &AppOptions::default(), &|| false).unwrap();
    let map = &graph.source_maps[0];
    assert!(map.present);
    assert!(map.sources.is_empty());
    assert!(map.sources_truncated);
    assert!(graph.limitations.iter().any(|(key, message)| key == "source_map_inventory_truncated" && message.contains("script-0.js")));
    let diff = diff_source_and_bundle(&source_graph(&["a.js"]), &graph, 100);
    assert_eq!(diff["unmatched_sources"], json!([]), "a refused map confirms nothing absent: {diff}");
    assert_eq!(diff["uncertain_sources"], json!([{"source": "a.js", "reason": "source_map_inventory_truncated"}]));
}

#[test]
fn a_map_refused_at_another_limit_leaves_its_sources_uncertain() {
    // More generated lines than the parser keeps by default.
    let lines = ";".repeat(craft_anything_formats::sourcemap::Limits::default().max_lines + 1);
    let graph = graph_with_maps(vec![json!({"version": 3, "sources": ["a.js"], "mappings": lines})]);
    assert!(graph.source_maps[0].sources_truncated);
    assert!(graph.limitations.iter().any(|(key, message)| key == "source_map_inventory_truncated" && message.contains("generated lines")));
    let diff = diff_source_and_bundle(&source_graph(&["a.js"]), &graph, 100);
    assert_eq!(diff["uncertain_sources"], json!([{"source": "a.js", "reason": "source_map_inventory_truncated"}]));
    // A malformed map is unreadable, not truncated.
    let broken = graph_with_maps(vec![json!({"version": 2, "sources": ["a.js"], "mappings": ""})]);
    assert!(!broken.source_maps[0].sources_truncated);
    assert!(broken.source_maps[0].unreadable);
}

fn graph_with(files: Vec<(&str, &[u8])>, listed_only: &[&str]) -> AppGraph {
    let files: Vec<_> = files.into_iter().map(|(path, bytes)| AppFile { path: path.into(), bytes: bytes.to_vec() }).collect();
    let mut listing: Vec<_> = files.iter().map(|file| (file.path.clone(), file.bytes.len() as u64)).collect();
    listing.extend(listed_only.iter().map(|path| ((*path).to_string(), 1)));
    analyze_application(&AppInput { files, listing, ..Default::default() }, &AppOptions::default(), &|| false).unwrap()
}

#[test]
fn a_malformed_or_unloaded_map_leaves_its_sources_uncertain() {
    let script = b"//# sourceMappingURL=script-0.js.map\n".as_slice();
    for (what, graph) in [
        ("malformed", graph_with(vec![("script-0.js", script), ("script-0.js.map", b"{\"version\":3,\"sources\":[")], &[])),
        ("not loaded", graph_with(vec![("script-0.js", script)], &["script-0.js.map"])),
        ("bad data URL", graph_with(vec![("script-0.js", b"//# sourceMappingURL=data:application/json;base64,!!!\n")], &[])),
        ("stand-alone, not loaded", graph_with(vec![("script-0.js", b"run();\n")], &["script-0.js.map"])),
    ] {
        let map = &graph.source_maps[0];
        assert!(map.unreadable && map.sources.is_empty() && !map.sources_truncated, "{what}: {map:?}");
        assert!(graph.limitations.iter().any(|(key, _)| key == "source_map_unreadable"), "{what}");
        let diff = diff_source_and_bundle(&source_graph(&["a.js"]), &graph, 100);
        assert_eq!(diff["unmatched_sources"], json!([]), "{what}: {diff}");
        assert_eq!(diff["uncertain_sources"], json!([{"source": "a.js", "reason": "source_map_unreadable"}]), "{what}");
        assert_eq!(diff["source_map_unreadable"], true, "{what}");
    }
}

#[test]
fn a_stand_alone_map_beside_its_script_is_read() {
    let map = serde_json::to_vec(&json!({"version": 3, "sources": ["src/a.js"], "mappings": ""})).unwrap();
    let graph = graph_with(vec![("dist/main.js", b"run();\n"), ("dist/main.js.map", map.as_slice())], &[]);
    let found = &graph.source_maps[0];
    assert_eq!((found.script.as_str(), found.sources.as_slice(), found.unreadable), ("dist/main.js", ["src/a.js".to_string()].as_slice(), false));
    let diff = diff_source_and_bundle(&source_graph(&["src/a.js", "src/b.js"]), &graph, 100);
    assert_eq!(diff["summary"]["matched"], 1, "{diff}");
    assert_eq!(diff["unmatched_sources"], json!(["src/b.js"]));
    assert_eq!(diff["source_map_unreadable"], false);
}

#[test]
fn other_map_files_are_listed_but_not_read() {
    let map = serde_json::to_vec(&json!({"version": 3, "sources": ["src/a.js"], "mappings": ""})).unwrap();
    // A game level, a leftover map with no script beside it, a broken map beside a page, and one not loaded.
    let graph = graph_with(
        vec![("levels/world.map", b"P3 10 10"), ("old/gone.js.map", map.as_slice()), ("index.html", b"<p>hi</p>"), ("index.html.map", b"{")],
        &["big/level.map"],
    );
    assert_eq!(graph.source_maps.len(), 4);
    assert!(graph.source_maps.iter().all(|m| m.sources.is_empty() && !m.unreadable && !m.sources_truncated));
    assert!(!graph.limitations.iter().any(|(key, _)| key.starts_with("source_map")));
    let diff = diff_source_and_bundle(&source_graph(&["src/a.js"]), &graph, 100);
    assert_eq!(diff["unmatched_sources"], json!(["src/a.js"]));
}

#[test]
fn a_map_listed_with_a_leading_dot_slash_is_read_once() {
    // How the ZIP reader lists an entry named `./dist/main.js.map` it loaded as `dist/main.js.map`.
    let map = serde_json::to_vec(&json!({"version": 3, "sources": ["src/a.js"], "mappings": ""})).unwrap();
    let files = vec![
        AppFile { path: "dist/main.js".into(), bytes: b"//# sourceMappingURL=main.js.map\n".to_vec() },
        AppFile { path: "dist/main.js.map".into(), bytes: map },
    ];
    let listing = vec![("./dist/main.js".to_string(), 1), ("./dist/main.js.map".to_string(), 1)];
    let graph = analyze_application(&AppInput { files, listing, ..Default::default() }, &AppOptions::default(), &|| false).unwrap();
    assert_eq!(graph.source_maps.len(), 1, "{:?}", graph.source_maps);
    assert_eq!(graph.source_maps[0].sources, ["src/a.js"]);
    assert!(!graph.source_maps[0].unreadable);
}

#[test]
fn a_map_listed_with_a_leading_dot_slash_but_not_loaded_is_unreadable() {
    let files = vec![AppFile { path: "dist/main.js".into(), bytes: b"//# sourceMappingURL=main.js.map\n".to_vec() }];
    // The reader trims every leading `./`, so `././` lists the same file.
    let listing = vec![("./dist/main.js".to_string(), 1), ("././dist/main.js.map".to_string(), 1)];
    let graph = analyze_application(&AppInput { files, listing, ..Default::default() }, &AppOptions::default(), &|| false).unwrap();
    assert_eq!(graph.source_maps.len(), 1, "{:?}", graph.source_maps);
    assert!(graph.source_maps[0].present && graph.source_maps[0].unreadable);
    let diff = diff_source_and_bundle(&source_graph(&["src/a.js"]), &graph, 100);
    assert_eq!(diff["uncertain_sources"], json!([{"source": "src/a.js", "reason": "source_map_unreadable"}]));
}

#[test]
fn graphs_saved_before_unreadable_maps_load() {
    let graph = graph_with_maps(vec![json!({"version": 3, "sources": ["a.js"], "mappings": ""})]);
    let mut saved = serde_json::to_value(&graph).unwrap();
    saved["source_maps"][0].as_object_mut().unwrap().remove("unreadable");
    let restored: AppGraph = serde_json::from_value(saved).unwrap();
    assert!(!restored.source_maps[0].unreadable);
}

#[test]
fn a_map_with_broken_mappings_or_section_offsets_keeps_its_sources() {
    let section = |line: u32, source: &str| json!({"offset": {"line": line, "column": 0}, "map": {"version": 3, "sources": [source], "mappings": "AAAA"}});
    for (what, map) in [
        ("bad mappings", json!({"version": 3, "sources": ["a.js"], "mappings": "!!!"})),
        ("sections out of order", json!({"version": 3, "sections": [section(1, "a.js"), section(0, "b.js")]})),
        (
            "bad mappings in a section",
            json!({"version": 3, "sections": [{"offset": {"line": 0, "column": 0}, "map": {"version": 3, "sources": ["a.js"], "mappings": "!"}}]}),
        ),
    ] {
        let graph = graph_with_maps(vec![map]);
        let found = &graph.source_maps[0];
        assert!(!found.unreadable && !found.sources_truncated && found.sources.first().is_some_and(|s| s == "a.js"), "{what}: {found:?}");
        assert!(graph.limitations.iter().any(|(key, _)| key == "source_map_mappings_unreadable"), "{what}");
        assert!(!graph.limitations.iter().any(|(key, _)| key == "source_map_unreadable"), "{what}");
        // The list is whole, so a source it doesn't name is missing, not uncertain.
        let diff = diff_source_and_bundle(&source_graph(&["a.js", "c.js"]), &graph, 100);
        assert_eq!(diff["unmatched_sources"], json!(["c.js"]), "{what}: {diff}");
        assert_eq!(diff["uncertain_sources"], json!([]), "{what}");
    }
    // A list the mappings show is short, and a section with neither a map nor a URL, say nothing
    // about the sources.
    for map in [
        json!({"version": 3, "mappings": "AAAA"}),
        json!({"version": 3, "sources": ["a.js"], "mappings": "ACAA"}),
        json!({"version": 3, "sources": ["a.js"], "mappings": "AAAA,ACAA,!"}),
        json!({"version": 3, "sections": [section(0, "a.js"), {"offset": {"line": 1, "column": 0}}]}),
    ] {
        let graph = graph_with_maps(vec![map.clone()]);
        assert!(graph.source_maps[0].unreadable && graph.source_maps[0].sources.is_empty(), "{map}");
        let diff = diff_source_and_bundle(&source_graph(&["c.js"]), &graph, 100);
        assert_eq!(diff["unmatched_sources"], json!([]), "{map}");
    }
}

/// A section that points at an external map adds no sources, so an otherwise unmatched reference
/// source may be in the part nobody read: it is uncertain, not confirmed absent.
#[test]
fn an_unread_external_section_leaves_its_sources_uncertain() {
    let graph = graph_with_maps(vec![json!({
        "version": 3,
        "sections": [
            {"offset": {"line": 0, "column": 0}, "map": {"version": 3, "sources": ["a.js"], "mappings": ""}},
            {"offset": {"line": 10, "column": 0}, "url": "vendor.js.map"}
        ]
    })]);
    let found = &graph.source_maps[0];
    assert_eq!(found.external_sections, 1, "{found:?}");
    assert!(!found.sources_truncated && !found.unreadable);
    assert!(graph.limitations.iter().any(|(key, message)| key == "source_map_external_sections" && message.contains("script-0.js")), "{:?}", graph.limitations);

    let diff = diff_source_and_bundle(&source_graph(&["a.js", "vendor.js"]), &graph, 100);
    assert_eq!(diff["summary"]["matched"], 1, "{diff}");
    assert_eq!(diff["unmatched_sources"], json!([]), "{diff}");
    assert_eq!(diff["uncertain_sources"], json!([{"source": "vendor.js", "reason": "source_map_external_sections"}]));
    assert_eq!(diff["source_map_external_sections"], true);

    // A graph saved before the field existed reads it as zero: no map claims an unread section.
    let mut saved = serde_json::to_value(&graph).unwrap();
    saved["source_maps"][0].as_object_mut().unwrap().remove("external_sections");
    let restored: AppGraph = serde_json::from_value(saved).unwrap();
    assert_eq!(restored.source_maps[0].external_sections, 0);
}
