//! Regression tests for trailing-dot segment stripping and case-insensitive extension detection in
//! `recover::safe_module_path` and `recover::recover_modules`.

use craft_anything_js::Limits;
use craft_anything_js::model::BundleModule;
use craft_anything_js::recover::{RecoveredModule, RewriteLevel, module_path_key, recover_modules, safe_module_path};
use std::collections::HashSet;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

/// Scoped temporary directory under `std::env::temp_dir()` cleaned up on drop.
struct TempDir(PathBuf);

impl TempDir {
    /// Create a fresh isolated directory for one test case.
    fn new(label: &str) -> Self {
        let id = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("craft-js-trailing-dots-{}-{}-{label}", std::process::id(), id));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp test directory");
        Self(dir)
    }

    /// Return the directory path.
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Build a synthetic `BundleModule` slice over `source` with the given id and sha256.
fn make_module(id: &str, sha256: &str, len: usize) -> BundleModule {
    BundleModule {
        id: id.to_string(),
        format: "webpack".to_string(),
        chunk_ids: Vec::new(),
        start: 0,
        end: len as u32,
        body: Some((0, len as u32)),
        line: 1,
        params: Vec::new(),
        deps: Vec::new(),
        sha256: sha256.to_string(),
        fingerprint: String::new(),
    }
}

/// Write every recovered module exclusively (`create_new(true)`) into `root`, mirroring `js_sources_restore`.
fn write_all_exclusively(root: &Path, recovered: &[RecoveredModule]) {
    for m in recovered {
        let mut path = root.to_path_buf();
        for comp in m.file_name.split('/') {
            path.push(comp);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("failed creating parent for {:?} ({}): {e}", m.id, m.file_name));
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("failed exclusive write for module {:?} at {}: {e}", m.id, m.file_name));
        f.write_all(m.text.as_bytes()).expect("write module text");
    }
}

/// Trailing dots on directory or leaf segments (including 120-char truncation landing on `.`) are stripped so Windows paths do not collide on disk.
#[test]
fn trailing_dot_segments_and_truncated_dot_boundaries_do_not_collide_on_disk() {
    let source = "globalThis.mod = 1;\n";
    let prefix_119 = "a".repeat(119);
    let cut_dot_dir = format!("{prefix_119}.ts/index.js");
    let plain_119_dir = format!("{prefix_119}/index.js");

    let modules = vec![
        make_module("pkg./app.js", "11111111aaaaaaaa", source.len()),
        make_module("pkg/app.js", "22222222bbbbbbbb", source.len()),
        make_module("src/dir.js./child.js", "33333333cccccccc", source.len()),
        make_module("src/dir.js", "44444444dddddddd", source.len()),
        make_module(&cut_dot_dir, "55555555eeeeeeee", source.len()),
        make_module(&plain_119_dir, "66666666ffffffff", source.len()),
        make_module("src/leaf.", "7777777711111111", source.len()),
        make_module("src/leaf", "8888888822222222", source.len()),
    ];

    let recovered = recover_modules(source, &modules, RewriteLevel::None, &Limits::default());
    assert_eq!(recovered.len(), modules.len());

    let dir = TempDir::new("trailing-dots");
    write_all_exclusively(dir.path(), &recovered);

    assert_eq!(recovered[0].file_name, "pkg/app.js");
    assert_eq!(recovered[1].file_name, "pkg/app.22222222.js");
    assert_eq!(recovered[2].file_name, "src/dir.js/child.js");
    assert_eq!(recovered[3].file_name, "src/dir.44444444.js");
    assert_eq!(recovered[4].file_name, format!("{prefix_119}/index.js"));
    assert_eq!(recovered[5].file_name, format!("{prefix_119}/index.66666666.js"));
    assert_eq!(recovered[6].file_name, "src/leaf.js");
    assert_eq!(recovered[7].file_name, "src/leaf.88888888.js");

    assert_eq!(safe_module_path("foo.", ""), "foo");
    assert_eq!(safe_module_path("a.../b...", ".js"), "a/b.js");
    assert_eq!(safe_module_path(".../.../", ".js"), "module.js");

    let keys: HashSet<String> = recovered.iter().map(|m| module_path_key(&m.file_name)).collect();
    assert_eq!(keys.len(), modules.len());

    let reverse_file_dir =
        vec![make_module("src/dir.js", "44444444dddddddd", source.len()), make_module("src/dir.js./child.js", "33333333cccccccc", source.len())];
    let rec_rev = recover_modules(source, &reverse_file_dir, RewriteLevel::None, &Limits::default());
    let dir_rev = TempDir::new("trailing-dots-rev");
    write_all_exclusively(dir_rev.path(), &rec_rev);
    assert_eq!(rec_rev[0].file_name, "src/dir.44444444.js");
    assert_eq!(rec_rev[1].file_name, "src/dir.js/child.js");
}

/// Uppercase and mixed-case known module extensions keep their extension instead of appending `.js`, and deduplicate against lowercase peers.
#[test]
fn uppercase_and_mixed_case_known_extensions_do_not_append_duplicate_js() {
    for (input, expected) in [
        ("src/PKG.JS", "src/PKG.JS"),
        ("src/esm.MJS", "src/esm.MJS"),
        ("src/common.CJS", "src/common.CJS"),
        ("src/types.TS", "src/types.TS"),
        ("src/App.TSX", "src/App.TSX"),
        ("src/View.JSX", "src/View.JSX"),
        ("src/data.JSON", "src/data.JSON"),
        ("src/theme.CSS", "src/theme.CSS"),
        ("src/Page.Vue", "src/Page.Vue"),
        ("src/Card.SVELTE", "src/Card.SVELTE"),
        (".JS", "JS.js"),
        ("src/.TS", "src/TS.js"),
        ("src/README.MD", "src/README.MD.js"),
    ] {
        assert_eq!(safe_module_path(input, ".js"), expected, "input: {input}");
    }

    let source = "globalThis.ext = 1;\n";
    let modules = vec![
        make_module("src/pkg.js", "aaaa1111bbbb2222", source.len()),
        make_module("src/PKG.JS", "cccc3333dddd4444", source.len()),
        make_module("src/App.TSX", "eeee5555ffff6666", source.len()),
        make_module("src/app.tsx", "7777888899990000", source.len()),
    ];

    let recovered = recover_modules(source, &modules, RewriteLevel::None, &Limits::default());
    let dir = TempDir::new("upper-ext");
    write_all_exclusively(dir.path(), &recovered);

    assert_eq!(recovered[0].file_name, "src/pkg.js");
    assert_eq!(recovered[1].file_name, "src/PKG.cccc3333.JS");
    assert_eq!(recovered[2].file_name, "src/App.TSX");
    assert_eq!(recovered[3].file_name, "src/app.77778888.tsx");
}

/// Ordinary module paths, leading-dot hidden files, and interior double dots remain unchanged.
#[test]
fn ordinary_module_paths_and_interior_dots_control() {
    assert_eq!(safe_module_path("webpack:///./src/index.ts", ".js"), "src/index.ts");
    assert_eq!(safe_module_path("src/foo..js", ".js"), "src/foo..js");
    assert_eq!(safe_module_path(".hidden", ".js"), "hidden.js");
}
