use craft_anything_js::{Limits, analyze, util::LineIndex};
/// All ECMAScript line terminators advance locations in parsed and fallback facts.
#[test]
fn all_line_terminators_locate_imports() {
    for sep in ["\r", "\u{2028}", "\u{2029}"] {
        let src = format!("const x = 'é';{sep}import value from './dep.js';");
        for limits in [Limits::default(), Limits { max_parse_bytes: 0, ..Limits::default() }] {
            let a = analyze("app.mjs", &src, &limits);
            assert_eq!(a.facts.lines, 2, "{sep:?}");
            assert_eq!(a.facts.imports[0].line, 2, "{sep:?}");
        }
    }
}
/// CRLF is one line ending; byte offsets still work after multibyte characters.
#[test]
fn mixed_terminators_and_offsets() {
    let src = "é\r\n中\rx\u{2028}y\u{2029}z\n";
    let index = LineIndex::new(src);
    assert_eq!(index.count(), 6);
    for (name, line) in [("é", 1), ("中", 2), ("x", 3), ("y", 4), ("z", 5)] {
        assert_eq!(index.line(src.find(name).unwrap() as u32), line);
    }
    assert_eq!(index.line(src.len() as u32), 6);
}
/// Existing LF and CRLF offsets and empty files remain unchanged.
#[test]
fn ordinary_line_endings_are_unchanged() {
    for s in ["a\nb", "a\r\nb"] {
        let i = LineIndex::new(s);
        assert_eq!(i.count(), 2);
        assert_eq!(i.line((s.len() - 1) as u32), 2);
    }
    assert_eq!(LineIndex::new("").count(), 1);
}
