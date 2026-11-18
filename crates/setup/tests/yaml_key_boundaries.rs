//! Regression tests for YAML mapping key boundary detection in `craft_anything_setup::yaml`.

use craft_anything_setup::yaml;
use serde_json::json;

/// Baseline block mappings with standard `key:` headers continue to round-trip comments.
#[test]
fn standard_block_mapping_edits_preserve_comments() {
    let input = "# top\nmcp_servers:\n  # existing\n  other:\n    command: keep\n";
    let entry = json!({"command": "craft-anything-cli", "args": ["mcp"]});
    let updated = yaml::set(input, "mcp_servers", "craft-anything", &entry).unwrap();
    assert!(updated.contains("# top") && updated.contains("# existing"));
    let parsed = yaml::parse(&updated).unwrap();
    assert_eq!(parsed["mcp_servers"]["other"]["command"], "keep");
    assert_eq!(parsed["mcp_servers"]["craft-anything"]["command"], "craft-anything-cli");
}

/// Top-level and child keys with spaces or tabs before `:` must update in place instead of duplicating blocks.
#[test]
fn spaced_colons_on_table_and_child_keys_edit_in_place() {
    let input = "model: fast\nmcp_servers :\n  other:\n    command: keep\n  craft-anything :\n    command: old\n";
    let entry = json!({"command": "new-cli", "enabled": true});
    let updated = yaml::set(input, "mcp_servers", "craft-anything", &entry).unwrap();
    assert_eq!(
        updated.lines().filter(|l| l.trim_start().starts_with("mcp_servers")).count(),
        1,
        "must reuse existing `mcp_servers :` block rather than appending a second top-level table:\n{updated}"
    );
    let parsed = yaml::parse(&updated).unwrap();
    assert_eq!(parsed["mcp_servers"]["other"]["command"], "keep");
    assert_eq!(parsed["mcp_servers"]["craft-anything"]["command"], "new-cli");

    let removed = yaml::remove(input, "mcp_servers", "craft-anything").expect("spaced key should be removable");
    let after = yaml::parse(&removed).unwrap();
    assert!(after["mcp_servers"].get("craft-anything").is_none());
    assert_eq!(after["mcp_servers"]["other"]["command"], "keep");
}

/// Quoted and tab-spaced table keys with empty flow values (`{}` / `~`) must expand into block mappings cleanly.
#[test]
fn quoted_table_headers_with_empty_flow_mappings_expand_in_place() {
    for header in ["\"mcp_servers\" : {} # empty", "mcp_servers\t: {} # tabbed", "'mcp_servers' : ~"] {
        let input = format!("model: fast\n{header}\n");
        let entry = json!({"command": "craft-anything-cli"});
        let updated = yaml::set(&input, "mcp_servers", "craft-anything", &entry).unwrap();
        let parsed = yaml::parse(&updated).unwrap();
        assert_eq!(parsed["mcp_servers"]["craft-anything"]["command"], "craft-anything-cli", "header: {header}\n{updated}");
        assert_eq!(parsed["mcp_servers"].as_object().unwrap().len(), 1);
    }
}

/// Unquoted sibling keys containing embedded colons (`craft-anything:dev:`) must not be mistaken for `craft-anything:`.
#[test]
fn embedded_colon_in_plain_sibling_key_is_not_clobbered_or_removed() {
    let input = "mcp_servers:\n  craft-anything:dev:\n    command: custom-dev\n";
    assert_eq!(
        yaml::parse(input).unwrap()["mcp_servers"]["craft-anything:dev"]["command"],
        "custom-dev",
        "YAML plain scalar rules treat `craft-anything:dev` as a single mapping key"
    );

    assert!(yaml::remove(input, "mcp_servers", "craft-anything").is_none(), "removing `craft-anything` must leave `craft-anything:dev` untouched");

    let entry = json!({"command": "prod-cli"});
    let updated = yaml::set(input, "mcp_servers", "craft-anything", &entry).unwrap();
    let parsed = yaml::parse(&updated).unwrap();
    assert_eq!(parsed["mcp_servers"]["craft-anything:dev"]["command"], "custom-dev", "{updated}");
    assert_eq!(parsed["mcp_servers"]["craft-anything"]["command"], "prod-cli", "{updated}");
}
