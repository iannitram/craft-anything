use craft_anything_formats::sourcemap::find_source_mapping_url;

/// An unfinished block hint cannot override an earlier complete annotation.
#[test]
fn ignores_unclosed_block_annotations() {
    let text = "//# sourceMappingURL=real.map\n/*# sourceMappingURL=unfinished.map";
    assert_eq!(find_source_mapping_url(text).as_deref(), Some("real.map"));
}

/// The first closing delimiter ends the comment even when another follows it.
#[test]
fn never_treats_text_after_the_block_close_as_part_of_the_url() {
    assert_eq!(find_source_mapping_url("/*# sourceMappingURL=first.map*/+second*/"), None);
}

/// Both comment markers work in complete blocks, and line comment URLs are unchanged.
#[test]
fn accepts_complete_block_annotations_and_line_controls() {
    for marker in ["/*#", "/*@"] {
        let text = format!("{marker} sourceMappingURL=app.map \t*/  ");
        assert_eq!(find_source_mapping_url(&text).as_deref(), Some("app.map"));
    }
    assert_eq!(find_source_mapping_url("//@ sourceMappingURL=path/*/").as_deref(), Some("path/*/"));
}

/// Ordinary comments after an annotation do not hide the URL.
#[test]
fn keeps_a_block_annotation_before_ordinary_comments() {
    for suffix in [" /* license */", " /* license */ /* notice */", " // build note"] {
        let text = format!("/*@ sourceMappingURL=app.map*/{suffix}");
        assert_eq!(find_source_mapping_url(&text).as_deref(), Some("app.map"), "{text}");
    }
}

/// In a comment-only line, the last valid annotation wins.
#[test]
fn keeps_the_last_annotation_on_a_comment_only_line() {
    let text = "/* license */ /*# sourceMappingURL=first.map*/ /*@ sourceMappingURL= */ //# sourceMappingURL=last.map";
    assert_eq!(find_source_mapping_url(text).as_deref(), Some("last.map"));
}

/// Code after the comment sequence prevents its URL from being used.
#[test]
fn rejects_code_after_the_trailing_comments() {
    let text = "/*# sourceMappingURL=app.map*/ /* license */ run();";
    assert_eq!(find_source_mapping_url(text), None);
}
