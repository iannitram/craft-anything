use craft_anything_formats::jsonc;
use serde_json::json;

/// Creating the first object keeps CRLF in comment-only and whitespace-only files.
#[test]
fn first_object_keeps_existing_line_endings() {
    for src in ["\r\n", "// settings\r\n", "\u{feff}// settings\r\n// tail"] {
        let output = jsonc::parse(src, false).unwrap().set(&["mcpServers", "example"], &json!({"command": "example"})).unwrap();
        assert!(output.starts_with(src));
        assert_eq!(output.matches('\n').count(), output.matches("\r\n").count(), "{output:?}");
        assert_eq!(jsonc::parse(&output, false).unwrap().get(&["mcpServers", "example", "command"]), Some(json!("example")));
    }
    assert!(jsonc::parse("// settings\r\n", true).is_err());
    let output = jsonc::parse("", false).unwrap().set(&["x"], &json!(1)).unwrap();
    assert!(!output.contains('\r'));
}
