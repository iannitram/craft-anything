use craft_anything_formats::sourcemap::find_source_mapping_url;

/// A JavaScript comment stops on CR, Unicode LS, or Unicode PS as well as LF.
#[test]
fn reads_the_last_map_after_each_ecmascript_line_ending() {
    for sep in ["\r", "\u{2028}", "\u{2029}", "\n", "\r\n"] {
        let js = format!("let answer = 42;{sep}//# sourceMappingURL=earlier.map{sep}//# sourceMappingURL=final.map{sep}// end{sep}");
        assert_eq!(find_source_mapping_url(&js).as_deref(), Some("final.map"), "{sep:?}");
    }
}

/// The 64-line window counts CRLF once, including real blank lines at EOF.
#[test]
fn counts_crlf_once_in_the_existing_lookup_window() {
    for (lines, expected) in [(64, Some("app.js.map")), (65, None)] {
        let js = format!("//# sourceMappingURL=app.js.map{}", "\r\n".repeat(lines));
        assert_eq!(find_source_mapping_url(&js).as_deref(), expected);
    }
}
