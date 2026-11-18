//! Regressions for Hermes YAML setup edits with spaced empty braces, YAML 1.2 null case
//! variants, one-line inline flow entries, safe refusal of multi-line flow entries under
//! `mcp_servers`, and preservation of sibling entries next to block scalars or quoted braces.

use craft_anything_setup::{ActionKind, Host, PlanRequest, apply, config, plan, uninstall, yaml};
use serde_json::json;

/// YAML 1.2 empty flow mappings with spaces (`{ }`) and case-variant nulls (`Null`, `NULL`)
/// parse as empty mappings and should be editable by `yaml::set`.
#[test]
fn yaml_set_accepts_spaced_empty_braces_and_case_variant_nulls() {
    let entry = json!({"command": "/opt/craft-anything-cli", "args": ["mcp"], "enabled": true});
    for (label, text) in [
        ("spaced braces", "model: big\nmcp_servers: { }\n"),
        ("spaced braces with comment", "model: big\nmcp_servers: {   } # empty for now\n"),
        ("titlecase Null", "model: big\nmcp_servers: Null\n"),
        ("uppercase NULL", "model: big\nmcp_servers: NULL # none yet\n"),
        ("child spaced braces", "model: big\nmcp_servers:\n  craft-anything: {   }\n"),
        ("child titlecase Null", "model: big\nmcp_servers:\n  craft-anything: Null\n"),
    ] {
        let updated = yaml::set(text, "mcp_servers", "craft-anything", &entry).unwrap_or_else(|e| panic!("{label}: expected yaml::set to succeed, got: {e}"));
        let parsed = yaml::parse(&updated).expect("updated YAML parses");
        assert_eq!(parsed["mcp_servers"]["craft-anything"], entry, "{label}");
        assert_eq!(parsed["model"], "big", "{label}");
        if text.contains('#') {
            assert!(updated.contains('#'), "{label} lost inline comment:\n{updated}");
        }
    }
}

/// Complete single-line flow siblings (such as `other: {command: npx, args: ["a"]}`) and
/// single-line flow `craft-anything` entries are delimited by indentation on their own line,
/// so `set` and `remove` succeed while leaving sibling lines byte-for-byte untouched.
#[test]
fn one_line_flow_siblings_and_entries_preserve_siblings_verbatim() {
    let entry = json!({"command": "/opt/craft-anything-cli", "args": ["mcp"], "enabled": true});
    let updated_entry = json!({"command": "/new/craft-anything-cli", "args": ["mcp"], "enabled": true});

    for (label, initial, sibling_line) in [
        ("one-line flow sibling", "model: big\nmcp_servers:\n  other: {command: npx, args: [\"a\"]}\n", "  other: {command: npx, args: [\"a\"]}\n"),
        (
            "nested flow sibling with comment",
            "mcp_servers:\n  other: {command: npx, args: [\"a\"], env: {FOO: \"a}b\"}} # keep me\n",
            "  other: {command: npx, args: [\"a\"], env: {FOO: \"a}b\"}} # keep me\n",
        ),
        ("tab after colon on flow sibling", "mcp_servers:\n  other:\t{command: npx, args: [\"a\"]}\n", "  other:\t{command: npx, args: [\"a\"]}\n"),
        (
            "one-line flow sibling referencing earlier anchor",
            "shared_args: &args [\"a\"]\nmcp_servers:\n  other: {command: npx, args: *args}\n",
            "  other: {command: npx, args: *args}\n",
        ),
        (
            "direct alias sibling referencing earlier server anchor",
            "shared_srv: &srv\n  command: npx\n  args: [\"a\"]\nmcp_servers:\n  other: *srv\n",
            "  other: *srv\n",
        ),
    ] {
        let added = yaml::set(initial, "mcp_servers", "craft-anything", &entry).unwrap_or_else(|e| panic!("{label}: initial set failed: {e}"));
        assert!(added.contains(sibling_line), "{label}: sibling changed after add:\n{added}");
        let parsed = yaml::parse(&added).expect("added YAML parses");
        assert_eq!(parsed["mcp_servers"]["craft-anything"], entry, "{label}");
        assert_eq!(parsed["mcp_servers"]["other"]["command"], "npx", "{label}");
        assert_eq!(parsed["mcp_servers"]["other"]["args"], json!(["a"]), "{label}");

        let replaced = yaml::set(&added, "mcp_servers", "craft-anything", &updated_entry).unwrap_or_else(|e| panic!("{label}: update set failed: {e}"));
        assert!(replaced.contains(sibling_line), "{label}: sibling changed after replace:\n{replaced}");
        assert_eq!(yaml::parse(&replaced).unwrap()["mcp_servers"]["craft-anything"], updated_entry, "{label}");

        let removed =
            yaml::try_remove(&replaced, "mcp_servers", "craft-anything").unwrap_or_else(|e| panic!("{label}: try_remove failed: {e}")).expect("entry removed");
        assert_eq!(removed, initial, "{label}: remove did not restore exact original bytes");
        assert_eq!(yaml::remove(&replaced, "mcp_servers", "craft-anything").as_deref(), Some(initial), "{label}");
    }

    // Our own entry written as a single-line flow map (including plain apostrophes) next to a sibling.
    let own_inline = "mcp_servers:\n  craft-anything: {command: old, note: don't}\n  other: {command: npx, args: [\"a\"]}\n";
    let replaced = yaml::set(own_inline, "mcp_servers", "craft-anything", &updated_entry).expect("replace single-line flow craft-anything");
    assert!(replaced.contains("  other: {command: npx, args: [\"a\"]}\n"), "{replaced}");
    assert_eq!(yaml::parse(&replaced).unwrap()["mcp_servers"]["craft-anything"], updated_entry);

    let removed_own = yaml::try_remove(own_inline, "mcp_servers", "craft-anything").expect("try_remove single-line flow craft-anything").expect("removed");
    assert_eq!(removed_own, "mcp_servers:\n  other: {command: npx, args: [\"a\"]}\n");
}

/// Multi-line flow entries under `mcp_servers` (whether on `craft-anything`, on a preceding
/// sibling, split across lines after a comma, or closed with `}`/`]` at child indentation)
/// must be refused with an actionable manual-edit error instead of guessing boundaries.
#[test]
fn yaml_refuses_flow_children_without_guessing_boundaries() {
    let entry = json!({"command": "/new/craft-anything-cli", "args": ["mcp"], "enabled": true});
    let cases = [
        (
            "multiline flow child",
            "model: big\nmcp_servers:\n  craft-anything: {\n    command: \"/old/craft-anything-cli\",\n    args: [\"mcp\"]\n  }\n  other:\n    command: npx\n",
        ),
        ("flow sibling before target", "mcp_servers:\n  other: {\n    command: \"npx\",\n    args: [\"a\"]\n  }\n  craft-anything:\n    command: old\n"),
        ("multiline flow sibling split after first field", "mcp_servers:\n  other: {command: npx,\n    args: [\"a\"]}\n  craft-anything:\n    command: old\n"),
        ("multiline flow array child", "mcp_servers:\n  craft-anything: [\n    \"item\"\n    ]\n  other:\n    command: npx\n"),
        (
            "explicit key flow value closing brace at child indent",
            "mcp_servers:\n  ? other\n  :\n    {\n      command: npx\n  }\n  craft-anything:\n    command: old\n",
        ),
        (
            "target flow value opened on next line and closed at child indent",
            "mcp_servers:\n  craft-anything:\n    {\n      command: old\n  }\n  other:\n    command: npx\n",
        ),
        ("quoted key unclosed flow brace", "mcp_servers:\n  \"other:prod\": {\n    command: \"npx\"\n  }\n  craft-anything:\n    command: old\n"),
    ];

    for (label, text) in cases {
        yaml::parse(text).unwrap_or_else(|e| panic!("{label}: fixture must be valid YAML: {e}"));
        let set_err = yaml::set(text, "mcp_servers", "craft-anything", &entry).expect_err(&format!("{label}: set should refuse multi-line flow child"));
        assert!(set_err.contains("by hand"), "{label}: unexpected error: {set_err}");

        let try_rm_err =
            yaml::try_remove(text, "mcp_servers", "craft-anything").expect_err(&format!("{label}: try_remove should refuse multi-line flow child"));
        assert!(try_rm_err.contains("by hand"), "{label}: unexpected try_remove error: {try_rm_err}");

        assert!(yaml::remove(text, "mcp_servers", "craft-anything").is_none(), "{label}: remove should return None");
    }
}

/// An empty-valued child entry whose flow block starts on the next line (`other:\n    {`), or any
/// line at child indentation beginning with `{`, `[`, `}`, `]`, or `,`, must be refused before any
/// edit so a nested `"craft-anything": ...` line at child indentation can never be mistaken for the
/// top-level server entry.
#[test]
fn next_line_flow_blocks_and_child_indent_flow_tokens_are_refused() {
    let entry = json!({"command": "/new/craft-anything-cli", "args": ["mcp"], "enabled": true});
    let nested_trap = "mcp_servers:\n  other:\n    {\n      command: npx,\n  \"craft-anything\": {command: nested}\n    ,\n  \"enabled\": true\n    }\n  craft-anything:\n    command: actual\n";
    let cases = [
        ("empty-value sibling with next-line flow brace and child-indent trap", nested_trap),
        (
            "empty-value sibling with next-line flow sequence",
            "mcp_servers:\n  other:\n    [\n      \"first\",\n      \"second\"\n    ]\n  craft-anything:\n    command: real\n",
        ),
        (
            "child-indent comma continuation inside next-line flow map",
            "mcp_servers:\n  other:\n    {\n      command: npx\n  ,\n      \"enabled\": true\n    }\n  craft-anything:\n    command: real\n",
        ),
        ("child-indent flow mapping key", "mcp_servers:\n  {a: 1}: {command: npx}\n  craft-anything:\n    command: real\n"),
        ("child-indent flow sequence key", "mcp_servers:\n  [\"a\"]: {command: npx}\n  craft-anything:\n    command: real\n"),
        ("next-line flow block after target entry", "mcp_servers:\n  craft-anything:\n    command: real\n  other:\n    {\n      command: npx\n    }\n"),
        (
            "anchored empty header opening next-line flow map",
            "mcp_servers:\n  other: &srv # anchor before flow\n    {\n      command: npx\n    }\n  craft-anything:\n    command: real\n",
        ),
    ];

    for (label, text) in cases {
        yaml::parse(text).unwrap_or_else(|e| panic!("{label}: fixture must be valid YAML: {e}"));
        let set_err = yaml::set(text, "mcp_servers", "craft-anything", &entry).expect_err(&format!("{label}: set must refuse"));
        assert!(set_err.contains("by hand"), "{label}: unexpected set error: {set_err}");

        let rm_err = yaml::try_remove(text, "mcp_servers", "craft-anything").expect_err(&format!("{label}: try_remove must refuse"));
        assert!(rm_err.contains("by hand"), "{label}: unexpected try_remove error: {rm_err}");
        assert!(yaml::remove(text, "mcp_servers", "craft-anything").is_none(), "{label}");
    }

    // End-to-end file preservation check on the nested-key trap.
    let dir = tempfile::tempdir().unwrap();
    let hermes_dir = dir.path().join("hermes");
    std::fs::create_dir_all(&hermes_dir).unwrap();
    let cfg = hermes_dir.join("config.yaml");
    let cli = dir.path().join("craft-anything-cli");
    std::fs::write(&cli, "").unwrap();
    let trap_owned = format!(
        "mcp_servers:\n  other:\n    {{\n      command: npx,\n  \"craft-anything\": {{command: nested}}\n    ,\n  \"enabled\": true\n    }}\n  craft-anything:\n    command: {}\n    args: [\"mcp\"]\n",
        json!(cli.join("stale").join("craft-anything-cli"))
    );
    std::fs::write(&cfg, &trap_owned).unwrap();

    let mut host = Host::with_home(dir.path());
    host.env.insert("HERMES_HOME".into(), hermes_dir.display().to_string());
    let req = PlanRequest { clients: vec!["hermes".into()], all_detected: false, command: vec![cli.display().to_string(), "mcp".into()], include_skill: false };
    let p = plan(&host, &req).unwrap();
    assert!(p.blocked, "{:?}", p.actions);
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), trap_owned);

    let u = uninstall(&host);
    assert_eq!(u.status, "blocked");
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), trap_owned);
}

/// Block mappings containing braces inside multi-line quoted strings, block scalars (`|`), or
/// plain scalars (`prefix{suffix`) are delimited safely by indentation alone so updating or
/// removing `craft-anything` always preserves sibling entries like `other`.
#[test]
fn block_scalars_and_quoted_or_plain_braces_preserve_siblings() {
    let entry = json!({"command": "/new/craft-anything-cli", "args": ["mcp"], "enabled": true});
    let cases = [
        (
            "multiline double-quoted brace",
            "mcp_servers:\n  craft-anything:\n    command: old\n    note: \"first\n      { second\"\n  other:\n    command: npx\n",
        ),
        ("literal block scalar brace", "mcp_servers:\n  craft-anything:\n    command: old\n    note: |\n      {\n  other:\n    command: npx\n"),
        ("plain scalar containing brace", "mcp_servers:\n  craft-anything:\n    command: old\n    note: prefix{suffix\n  other:\n    command: npx\n"),
        (
            "nested flow mapping inside block child",
            "mcp_servers:\n  craft-anything:\n    command: old\n    env: {\n      FOO: \"1\"\n    }\n  other:\n    command: npx\n",
        ),
        (
            "sibling literal scalar starting with brace",
            "mcp_servers:\n  note: |\n    {literal text\n  craft-anything:\n    command: old\n  other:\n    command: npx\n",
        ),
        (
            "sibling folded scalar starting with bracket",
            "mcp_servers:\n  note: >\n    [folded text\n  craft-anything:\n    command: old\n  other:\n    command: npx\n",
        ),
        (
            "sibling anchored strip literal scalar starting with brace",
            "mcp_servers:\n  note: &n |- # comment\n    {literal text\n  craft-anything:\n    command: old\n  other:\n    command: npx\n",
        ),
        (
            "sibling explicit-indent block scalar starting with brace",
            "mcp_servers:\n  note: |2\n    {literal text\n  craft-anything:\n    command: old\n  other:\n    command: npx\n",
        ),
    ];

    for (label, text) in cases {
        let updated = yaml::set(text, "mcp_servers", "craft-anything", &entry).unwrap_or_else(|e| panic!("{label}: set failed: {e}"));
        let parsed = yaml::parse(&updated).expect("updated YAML parses");
        assert_eq!(parsed["mcp_servers"]["craft-anything"], entry, "{label}");
        assert_eq!(parsed["mcp_servers"]["other"]["command"], "npx", "{label}: lost other on set");

        let removed =
            yaml::try_remove(text, "mcp_servers", "craft-anything").unwrap_or_else(|e| panic!("{label}: try_remove failed: {e}")).expect("entry removed");
        let parsed_removed = yaml::parse(&removed).expect("removed YAML parses");
        assert!(parsed_removed["mcp_servers"].get("craft-anything").is_none(), "{label}");
        assert_eq!(parsed_removed["mcp_servers"]["other"]["command"], "npx", "{label}: lost other on remove");
    }
}

/// End-to-end Hermes `plan`, `apply`, and `uninstall` succeed with spaced empty braces and
/// single-line flow siblings, and refuse to overwrite or delete when an entry spans multiple
/// lines in flow style.
#[test]
fn hermes_setup_works_with_spaced_braces_and_refuses_multiline_flow_entry() {
    let dir = tempfile::tempdir().unwrap();
    let hermes_dir = dir.path().join("hermes");
    std::fs::create_dir_all(&hermes_dir).unwrap();
    let cfg = hermes_dir.join("config.yaml");
    std::fs::write(&cfg, "model: big\nmcp_servers: { } # configured empty\n").unwrap();

    let mut host = Host::with_home(dir.path());
    host.env.insert("HERMES_HOME".into(), hermes_dir.display().to_string());
    let cli = dir.path().join("craft-anything-cli");
    std::fs::write(&cli, "").unwrap();
    let cmd = vec![cli.display().to_string(), "mcp".into()];

    let req = PlanRequest { clients: vec!["hermes".into()], all_detected: false, command: cmd, include_skill: false };
    let p = plan(&host, &req).unwrap();
    assert!(!p.blocked, "{:?}", p.actions);
    assert_eq!(apply(&p).status, "configured");

    // A one-line flow sibling works across setup plan, apply, and uninstall without altering the sibling.
    let one_line_sibling = "model: big\nmcp_servers:\n  other: {command: npx, args: [\"a\"]}\n";
    std::fs::write(&cfg, one_line_sibling).unwrap();
    let p_sibling = plan(&host, &req).unwrap();
    assert!(!p_sibling.blocked, "{:?}", p_sibling.actions);
    assert_eq!(apply(&p_sibling).status, "configured");
    assert!(std::fs::read_to_string(&cfg).unwrap().contains("  other: {command: npx, args: [\"a\"]}\n"));
    let u_sibling = uninstall(&host);
    assert_eq!(u_sibling.status, "removed");
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), one_line_sibling);

    // Rewrite our entry as multi-line flow alongside a sibling, then verify plan/uninstall refuse without altering the file.
    let stale_cli = dir.path().join("old").join("craft-anything-cli");
    let flow_cfg = format!(
        "model: big\nmcp_servers:\n  craft-anything: {{\n    command: {},\n    args: [\"mcp\"],\n    enabled: true\n  }}\n  other:\n    command: npx\n",
        json!(stale_cli)
    );
    std::fs::write(&cfg, &flow_cfg).unwrap();
    let hermes_client = craft_anything_setup::clients(&host).into_iter().find(|c| c.id == "hermes").unwrap();
    assert!(config::read_entry(&hermes_client, &std::fs::read_to_string(&cfg).unwrap()).unwrap().is_some());

    let p2 = plan(&host, &req).unwrap();
    assert_eq!(p2.actions.len(), 1);
    assert_eq!(p2.actions[0].action, ActionKind::Invalid);
    assert!(p2.actions[0].reason.as_deref().is_some_and(|r| r.contains("by hand")), "{:?}", p2.actions[0].reason);
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), flow_cfg);

    let u = uninstall(&host);
    assert_eq!(u.status, "blocked");
    assert_eq!(std::fs::read_to_string(&cfg).unwrap(), flow_cfg);
}
