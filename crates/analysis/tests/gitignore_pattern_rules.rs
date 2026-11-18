//! Regression tests for `.gitignore` pattern escaping, trailing spaces, globstar boundaries,
//! and ignored parent-directory propagation in `js_source::Ignore`.

use craft_anything_analysis::js_source::Ignore;

/// Leading backslashes escape wildcards (`\*`, `\[`, `\?`), `#`, and `!`, while a dangling trailing `\` is rejected.
#[test]
fn leading_backslash_escapes_wildcards_comments_and_negation() {
    let mut ig = Ignore::default();
    ig.add_file("", "\\*temp.js\n\\[draft\\].js\n\\?mark.js\n\\#hash.js\n\\!bang.js\ndangling\\\n");

    assert!(ig.is_ignored("*temp.js", false));
    assert!(!ig.is_ignored("mytemp.js", false), "escaped leading * must not act as wildcard");

    assert!(ig.is_ignored("[draft].js", false));
    assert!(!ig.is_ignored("d.js", false), "escaped leading [ must not open character class");

    assert!(ig.is_ignored("?mark.js", false));
    assert!(!ig.is_ignored("xmark.js", false), "escaped leading ? must not match arbitrary char");

    assert!(ig.is_ignored("#hash.js", false));
    assert!(ig.is_ignored("!bang.js", false));
    assert!(!ig.is_ignored("dangling", false), "trailing unescaped backslash is invalid in gitignore");
}

/// Unescaped trailing spaces are stripped while `\ ` preserves one trailing space and tabs stay literal.
#[test]
fn trailing_spaces_strip_when_unquoted_and_keep_escaped_space_or_tab() {
    let mut ig = Ignore::default();
    ig.add_file("", "trail\\ \nunquoted   \nmulti\\   \nslash\\\\ \ntab\t\n");

    assert!(ig.is_ignored("trail ", false));
    assert!(!ig.is_ignored("trail", false), "escaped trailing space must not match trim-stripped name");

    assert!(ig.is_ignored("unquoted", false));
    assert!(!ig.is_ignored("unquoted   ", false));

    assert!(ig.is_ignored("multi ", false));
    assert!(!ig.is_ignored("multi   ", false));

    assert!(ig.is_ignored("slash\\", false));
    assert!(!ig.is_ignored("slash\\ ", false));

    assert!(ig.is_ignored("tab\t", false));
    assert!(!ig.is_ignored("tab", false), "trailing tab is literal in gitignore");
}

/// Consecutive stars cross `/` only when forming an entire slash-bounded segment (`**/`, `/**`, `/**/`, `**`).
#[test]
fn double_star_crosses_slashes_only_as_a_full_path_segment() {
    let mut ig = Ignore::default();
    ig.add_file("", "a**b.js\na**/b.js\na/**b.js\np/*/q.js\n**/deep.js\n**/**/chained.js\nsub/**\nx/**/y.js\n***/c.js\na/***/d.js\n");

    assert!(ig.is_ignored("ab.js", false));
    assert!(ig.is_ignored("axxb.js", false));
    assert!(!ig.is_ignored("a/x/b.js", false), "inline a**b.js and a**/b.js must not cross /");

    assert!(ig.is_ignored("a/b.js", false));
    assert!(ig.is_ignored("ax/b.js", false));
    assert!(ig.is_ignored("a/xb.js", false));
    assert!(!ig.is_ignored("a/x/xb.js", false), "inline a/**b.js must not cross /");
    assert!(ig.is_ignored("p/mid/q.js", false));
    assert!(!ig.is_ignored("p/q.js", false), "single * segment requires one directory");
    assert!(!ig.is_ignored("p/m1/m2/q.js", false), "single * segment must not cross /");

    assert!(ig.is_ignored("deep.js", false));
    assert!(ig.is_ignored("p/q/deep.js", false));
    assert!(ig.is_ignored("chained.js", false), "consecutive **/**/ matches zero directories");
    assert!(ig.is_ignored("p/chained.js", false));
    assert!(ig.is_ignored("sub/f.js", false));
    assert!(ig.is_ignored("sub/a/b.js", false));
    assert!(ig.is_ignored("x/y.js", false));
    assert!(ig.is_ignored("x/m/n/y.js", false));
    assert!(ig.is_ignored("c.js", false));
    assert!(ig.is_ignored("m/n/c.js", false));
    assert!(ig.is_ignored("a/d.js", false));
    assert!(ig.is_ignored("a/m/n/d.js", false));

    let mut neg_tail = Ignore::default();
    neg_tail.add_file("", "*.log\n!sub/**\n");
    assert!(neg_tail.is_ignored("top.log", false));
    assert!(!neg_tail.is_ignored("sub/a/b.log", false), "!sub/** must cross / to re-include nested log");

    let mut stand_alone = Ignore::default();
    stand_alone.add_file("", "**\n!dir/\n!dir/keep.js\n");
    assert!(stand_alone.is_ignored("other.js", false));
    assert!(!stand_alone.is_ignored("dir/keep.js", false));

    let mut escaped_slash = Ignore::default();
    escaped_slash.add_file("", "a/**\\/b.js\n");
    assert!(escaped_slash.is_ignored("a/x/y/b.js", false), "a/**\\/b.js must match nested paths across /");
    assert!(!escaped_slash.is_ignored("a/b.js", false), "escaped slash after ** preserves separate literal slash token");
}

/// Once an ancestor directory is ignored, all descendants are ignored and `!` child rules cannot re-include them unless the directory itself is unignored.
#[test]
fn ignored_parent_directories_propagate_to_descendants_and_block_child_reinclude() {
    let mut ig = Ignore::default();
    ig.add_file("", "ignored_dir/\n!ignored_dir/keep.js\n/dist\ndir1/*\n!dir1/keep.js\ndir2/\n!dir2/\ndir2/drop.js\n");

    assert!(ig.is_ignored("ignored_dir/other.js", false));
    assert!(ig.is_ignored("ignored_dir/keep.js", false), "cannot re-include file under excluded parent directory");

    assert!(ig.is_ignored("dist/app.js", false));
    assert!(!ig.is_ignored("other/dist/app.js", false));

    assert!(!ig.is_ignored("dir1/keep.js", false), "dir1 itself is not ignored so !dir1/keep.js applies");
    assert!(ig.is_ignored("dir1/drop.js", false));

    assert!(!ig.is_ignored("dir2/anything.js", false), "!dir2/ unignores the directory itself");
    assert!(ig.is_ignored("dir2/drop.js", false));

    let mut sub = Ignore::default();
    sub.add_file("", "sub/**\n!sub/keep.js\n!sub/nested/keep.js\n");
    assert!(!sub.is_ignored("sub/keep.js", false), "sub/** does not exclude sub itself");
    assert!(sub.is_ignored("sub/other.js", false));
    assert!(sub.is_ignored("sub/nested/keep.js", false), "sub/** excludes sub/nested directory");
}

/// Ordinary basename globs, `!` negations, and nested directory `.gitignore` scopes (including `.` and `./pkg`) retain existing behavior.
#[test]
fn nested_gitignore_and_ordinary_rules_control() {
    let mut ig = Ignore::default();
    ig.add_file(".", "*.log\n!keep.log\n[a-c]?.txt\n");
    ig.add_file("./pkg", "local.tmp\n");
    ig.add_file("a//b", "deep.tmp\n");

    assert!(ig.is_ignored("./a/b/debug.log", false));
    assert!(!ig.is_ignored("a/b/keep.log", false));
    assert!(ig.is_ignored("bx.txt", false));
    assert!(!ig.is_ignored("dx.txt", false));
    assert!(ig.is_ignored("./pkg/local.tmp", false));
    assert!(ig.is_ignored("pkg/local.tmp", false));
    assert!(!ig.is_ignored("local.tmp", false));
    assert!(ig.is_ignored("a//b/deep.tmp", false));
}
