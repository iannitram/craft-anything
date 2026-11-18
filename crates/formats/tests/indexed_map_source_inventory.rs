use craft_anything_formats::sourcemap::{InventoryLimits, Limits, parse, parse_sources};

/// Only real section sources use inventory slots, and each map keeps its own root.
#[test]
fn nested_index_fields_do_not_displace_section_sources() {
    let json = br#"{
        "version":3, "file":"generated.js", "sourceRoot":"ignored", "sources":["ghost.js"],
        "sections":[{"offset":{"line":0,"column":0}, "map":{
            "version":3, "sources":["nested-ghost.js"],
            "sections":[{"offset":{"line":0,"column":0}, "map":{
                "version":3, "sourceRoot":"src", "sources":["actual.js"], "mappings":"AAAA"
            }}]
        }}]
    }"#;
    for read in [parse, parse_sources] {
        let map = read(json, Limits::default()).unwrap();
        assert_eq!(map.file.as_deref(), Some("generated.js"));
        let inventory = map.source_inventory(InventoryLimits { max_sources: 1, max_bytes: 13 });
        assert_eq!(inventory.sources, ["src/actual.js"]);
        assert!(!inventory.truncated);
    }
    let map = parse(json, Limits::default()).unwrap();
    assert_eq!(map.original_positions(0, 0)[0].source.as_deref(), Some("src/actual.js"));
}

/// An empty section list supplies no sources even if the index has ordinary map fields.
#[test]
fn empty_index_has_no_sources_or_names() {
    let map = parse(br#"{"version":3,"sources":["ghost.js"],"names":["ghost"],"sections":[]}"#, Limits::default()).unwrap();
    assert!(map.all_sources().is_empty());
    assert!(map.names.is_empty());
    assert!(map.original_positions(0, 0).is_empty());
}

/// The same fields still work on a regular map.
#[test]
fn regular_map_keeps_sources_and_names() {
    let map = parse(br#"{"version":3,"sources":["real.js"],"names":["real"],"mappings":"AAAAA"}"#, Limits::default()).unwrap();
    assert_eq!(map.all_sources(), ["real.js"]);
    assert_eq!(map.original_positions(0, 0)[0].name.as_deref(), Some("real"));
}
