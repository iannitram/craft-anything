use craft_anything_formats::jsonc::parse;
use serde_json::json;

/// Inserting into a CR-only configuration keeps its line endings and indentation.
#[test]
fn insertion_preserves_cr_only_lines() {
    let text = "{\r\t\"keep\": 1\r}\r";
    let out = parse(text, false).unwrap().set(&["new"], &json!({"x": 2})).unwrap();
    assert!(!out.contains('\n'), "{out:?}");
    assert!(out.contains("\r\t\"new\":"), "{out:?}");
    assert_eq!(parse(&out, false).unwrap().get(&["new", "x"]), Some(json!(2)));
}

/// Comment-only CR documents receive an object without changing newline style.
#[test]
fn first_object_preserves_cr_only_lines() {
    let text = "// settings\r";
    let out = parse(text, false).unwrap().set(&["port"], &json!(42)).unwrap();
    assert_eq!(out, "// settings\r{\r  \"port\": 42\r}\r");
}

/// Removing a whole member line also removes that CR line, retaining adjacent comments.
#[test]
fn removal_preserves_cr_only_layout() {
    let text = "{\r  \"remove\": 1,\r  // keep this\r  \"keep\": 2\r}\r";
    let out = parse(text, false).unwrap().remove(&["remove"]).unwrap();
    assert_eq!(out, "{\r  // keep this\r  \"keep\": 2\r}\r");
}

/// Existing LF and CRLF documents still retain their original newline convention.
#[test]
fn other_line_endings_are_unchanged() {
    for eol in ["\n", "\r\n"] {
        let text = format!("{{{eol}  \"keep\": 1{eol}}}{eol}");
        let out = parse(&text, true).unwrap().set(&["new"], &json!(2)).unwrap();
        assert_eq!(out, format!("{{{eol}  \"keep\": 1,{eol}  \"new\": 2{eol}}}{eol}"));
    }
}
