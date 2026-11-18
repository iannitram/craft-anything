//! Shared span-preserving JSONC editing; file admission and write policy stay in the app.
pub use craft_jsonc::{Doc, Member, Node, NodeKind, parse};

#[cfg(test)]
#[path = "jsonc_legacy.rs"]
mod legacy;

#[cfg(test)]
mod parity_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn shared_parse_and_edits_match_original_bytes() {
        let samples = [
            "",
            "{}",
            "// only\r",
            "// only\r\n",
            "\u{feff}{}",
            "{\"a\":1,\"a\":2}",
            "{\"a\":{\"b\":3},}",
            "{\r  \"a\": 1,\r}\r",
            "{\r\n  \"a\": 1\r\n}\r\n",
            "{\"é\":\"界\"}",
            "[]",
            "{",
            "{\"a\": NaN}",
        ];
        for text in samples {
            for strict in [false, true] {
                match (legacy::parse(text, strict), parse(text, strict)) {
                    (Ok(old), Ok(new)) => {
                        assert_eq!(old.to_value(), new.to_value());
                        for path in [vec![], vec!["a"], vec!["a", "b"], vec!["missing"], vec!["é"]] {
                            assert_eq!(old.get(&path), new.get(&path));
                            assert_eq!(old.remove(&path), new.remove(&path));
                            for value in [json!(null), json!([1, 2]), json!({"x":"界"}), json!(7)] {
                                assert_eq!(old.set(&path, &value), new.set(&path, &value));
                            }
                        }
                    }
                    (Err(old), Err(new)) => assert_eq!(old, new),
                    _ => panic!("JSONC parse parity mismatch"),
                }
            }
        }
        for depth in [127, 128, 129, 256] {
            let text = format!("{}0{}", "{\"x\":".repeat(depth), "}".repeat(depth));
            assert_eq!(legacy::parse(&text, false).map(|d| d.to_value()), parse(&text, false).map(|d| d.to_value()));
        }
    }
}
