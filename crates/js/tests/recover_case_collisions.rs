//! Regression tests for case-insensitive path deduplication in `recover_modules`.

use craft_anything_js::Limits;
use craft_anything_js::model::BundleModule;
use craft_anything_js::recover::{RecoveredModule, RewriteLevel, ancestor_dir_keys, module_path_key, recover_modules};
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
        let dir = std::env::temp_dir().join(format!("craft-js-recover-{}-{}-{label}", std::process::id(), id));
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
fn make_module(id: &str, sha256: &str, start: usize, end: usize) -> BundleModule {
    BundleModule {
        id: id.to_string(),
        format: "webpack".to_string(),
        chunk_ids: Vec::new(),
        start: start as u32,
        end: end as u32,
        body: Some((start as u32, end as u32)),
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
            std::fs::create_dir_all(parent).expect("create parent directory");
        }
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("failed exclusive write for module {:?} at {}: {e}", m.id, m.file_name));
        f.write_all(m.text.as_bytes()).expect("write module text");
    }
}

/// Modules differing only in ASCII case receive distinct case-folded paths and write without `create_new` rejection.
#[test]
fn case_only_module_ids_keep_distinct_paths_and_write_exclusively() {
    let s0 = "globalThis.m0 = 10;\n";
    let s1 = "globalThis.m1 = 20;\n";
    let s2 = "globalThis.m2 = 30;\n";
    let s3 = "globalThis.m3 = 40;\n";
    let src = format!("{s0}{s1}{s2}{s3}");

    let o1 = s0.len();
    let o2 = o1 + s1.len();
    let o3 = o2 + s2.len();
    let o4 = o3 + s3.len();

    let modules = vec![
        make_module("X.CA978112.ts", "11111111aaaaaaaa", 0, o1),
        make_module("x.ca978112.ts", "22222222bbbbbbbb", o1, o2),
        make_module("src/App.js", "33333333cccccccc", o2, o3),
        make_module("src/app.js", "ca978112dddddddd", o3, o4),
    ];

    let recovered = recover_modules(&src, &modules, RewriteLevel::None, &Limits::default());
    assert_eq!(recovered.len(), 4);

    let dir = TempDir::new("ascii-case");
    write_all_exclusively(dir.path(), &recovered);

    assert_eq!(recovered[0].file_name, "X.CA978112.ts");
    assert_eq!(recovered[1].file_name, "x.ca978112.22222222.ts");
    assert_eq!(recovered[2].file_name, "src/App.js");
    assert_eq!(recovered[3].file_name, "src/app.ca978112.js");

    let folded: HashSet<String> = recovered.iter().map(|m| module_path_key(&m.file_name)).collect();
    assert_eq!(folded.len(), 4, "case-folded paths must be unique: {:?}", recovered.iter().map(|m| &m.file_name).collect::<Vec<_>>());
}

/// Disambiguated `.hash` and `.hash.occurrence` names also check case-insensitive collisions against earlier and later modules.
#[test]
fn generated_hash_and_occurrence_suffixes_avoid_case_collisions() {
    let source = "globalThis.answer = 42;";
    let modules = vec![
        make_module("X.CA978112.js", "11111111aaaaaaaa", 0, source.len()),
        make_module("X.js", "22222222bbbbbbbb", 0, source.len()),
        make_module("x.js", "ca978112cccccccc", 0, source.len()),
        make_module("X.CA978112.2.js", "33333333dddddddd", 0, source.len()),
    ];

    let recovered = recover_modules(source, &modules, RewriteLevel::None, &Limits::default());
    assert_eq!(recovered.len(), 4);

    let dir = TempDir::new("suffix-case");
    write_all_exclusively(dir.path(), &recovered);

    assert_eq!(recovered[0].file_name, "X.CA978112.js");
    assert_eq!(recovered[1].file_name, "X.js");
    assert_eq!(recovered[2].file_name, "x.ca978112.2.js");
    assert_eq!(recovered[3].file_name, "X.CA978112.2.33333333.js");

    let folded: HashSet<String> = recovered.iter().map(|m| module_path_key(&m.file_name)).collect();
    assert_eq!(folded.len(), 4);
}

/// Non-ASCII case pairs (including Greek `ς`/`σ`/`Σ`, `K`/`K`, `Å`/`Å`, `ı`/`I`, Hangul Jamo, and APFS `ß`/`ss`/`ẞ` and `ﬁ`/`fi`) get distinct paths.
#[test]
fn unicode_case_variants_including_final_sigma_get_distinct_paths() {
    let source = "globalThis.ok = 1;";
    let modules = vec![
        make_module("src/Über.js", "aaaa000011112222", 0, source.len()),
        make_module("src/über.js", "bbbb000011112222", 0, source.len()),
        make_module("src/ΟΣ.js", "cccc000011112222", 0, source.len()),
        make_module("src/οσ.js", "dddd000011112222", 0, source.len()),
        make_module("src/ος.js", "eeee000011112222", 0, source.len()),
        make_module("src/\u{212a}.js", "1212000011112222", 0, source.len()),
        make_module("src/K.js", "3434000011112222", 0, source.len()),
        make_module("src/\u{212b}.js", "5656000011112222", 0, source.len()),
        make_module("src/\u{00c5}.js", "7878000011112222", 0, source.len()),
        make_module("src/straße.js", "ffff000011112222", 0, source.len()),
        make_module("src/STRASSE.js", "9999000011112222", 0, source.len()),
        make_module("src/STRA\u{1e9e}E.js", "8888000011112222", 0, source.len()),
        make_module("src/\u{ac00}.js", "a1a1000011112222", 0, source.len()),
        make_module("src/\u{1100}\u{1161}.js", "a2a2000011112222", 0, source.len()),
        make_module("src/\u{ac01}.js", "a3a3000011112222", 0, source.len()),
        make_module("src/\u{1100}\u{1161}\u{11a8}.js", "a4a4000011112222", 0, source.len()),
        make_module("src/\u{d7a3}.js", "a5a5000011112222", 0, source.len()),
        make_module("src/\u{1112}\u{1175}\u{11c2}.js", "a6a6000011112222", 0, source.len()),
        make_module("src/\u{0131}.js", "b1b1000011112222", 0, source.len()),
        make_module("src/I.js", "b2b2000011112222", 0, source.len()),
        make_module("src/\u{fb01}le.js", "c1c1000011112222", 0, source.len()),
        make_module("src/file.js", "c2c2000011112222", 0, source.len()),
    ];

    let recovered = recover_modules(source, &modules, RewriteLevel::None, &Limits::default());
    assert_eq!(recovered.len(), 22);

    let dir = TempDir::new("unicode-case");
    write_all_exclusively(dir.path(), &recovered);

    assert_eq!(recovered[0].file_name, "src/Über.js");
    assert_eq!(recovered[1].file_name, "src/über.bbbb0000.js");
    assert_eq!(recovered[2].file_name, "src/ΟΣ.js");
    assert_eq!(recovered[3].file_name, "src/οσ.dddd0000.js");
    assert_eq!(recovered[4].file_name, "src/ος.eeee0000.js");
    assert_eq!(recovered[5].file_name, "src/\u{212a}.js");
    assert_eq!(recovered[6].file_name, "src/K.34340000.js");
    assert_eq!(recovered[7].file_name, "src/\u{212b}.js");
    assert_eq!(recovered[8].file_name, "src/\u{00c5}.78780000.js");
    assert_eq!(recovered[9].file_name, "src/straße.js");
    assert_eq!(recovered[10].file_name, "src/STRASSE.99990000.js");
    assert_eq!(recovered[11].file_name, "src/STRA\u{1e9e}E.88880000.js");
    assert_eq!(recovered[12].file_name, "src/\u{ac00}.js");
    assert_eq!(recovered[13].file_name, "src/\u{1100}\u{1161}.a2a20000.js");
    assert_eq!(recovered[14].file_name, "src/\u{ac01}.js");
    assert_eq!(recovered[15].file_name, "src/\u{1100}\u{1161}\u{11a8}.a4a40000.js");
    assert_eq!(recovered[16].file_name, "src/\u{d7a3}.js");
    assert_eq!(recovered[17].file_name, "src/\u{1112}\u{1175}\u{11c2}.a6a60000.js");
    assert_eq!(recovered[18].file_name, "src/\u{0131}.js");
    assert_eq!(recovered[19].file_name, "src/I.b2b20000.js");
    assert_eq!(recovered[20].file_name, "src/\u{fb01}le.js");
    assert_eq!(recovered[21].file_name, "src/file.c2c20000.js");

    let shared_keys: HashSet<String> = recovered.iter().map(|m| module_path_key(&m.file_name)).collect();
    assert_eq!(shared_keys.len(), 22);
    assert_eq!(module_path_key("src/straße.js"), "SRC/STRASSE.JS");
    assert_eq!(module_path_key("src/ος.js"), "SRC/ΟΣ.JS");
    assert_eq!(module_path_key("src/\u{212a}.js"), "SRC/K.JS");
    assert_eq!(module_path_key("src/\u{212b}.js"), "SRC/\u{00c5}.JS");
    assert_eq!(module_path_key("src/\u{0130}.js"), module_path_key("src/i\u{0307}.js"));
    assert_eq!(module_path_key("src/\u{ac00}.js"), module_path_key("src/\u{1100}\u{1161}.js"));
    assert_eq!(module_path_key("src/\u{ac01}.js"), module_path_key("src/\u{1100}\u{1161}\u{11a8}.js"));
    assert_eq!(module_path_key("src/\u{d7a3}.js"), module_path_key("src/\u{1112}\u{1175}\u{11c2}.js"));
    assert_eq!(module_path_key("\u{abff}\u{d7a4}"), "\u{abff}\u{d7a4}");
    assert_eq!(ancestor_dir_keys("app.js"), Vec::<String>::new());
    assert_eq!(ancestor_dir_keys("./src//PKG.JS/child.js"), vec!["SRC".to_string(), "SRC/PKG.JS".to_string()]);
    assert_eq!(ancestor_dir_keys("/src/PKG.JS/child.js"), vec!["SRC".to_string(), "SRC/PKG.JS".to_string()]);
}

/// File/parent-directory collisions (`src/pkg.js` vs `src/PKG.JS/child.js`) are disambiguated in either order.
#[test]
fn file_and_parent_directory_conflicts_resolve_in_either_order() {
    let source = "globalThis.pkg = 1;";

    let forward = vec![make_module("src/pkg.js", "1111aaaa2222bbbb", 0, source.len()), make_module("src/PKG.JS/child.js", "3333cccc4444dddd", 0, source.len())];
    let rec_fwd = recover_modules(source, &forward, RewriteLevel::None, &Limits::default());
    let dir_fwd = TempDir::new("file-dir-fwd");
    write_all_exclusively(dir_fwd.path(), &rec_fwd);
    assert_eq!(rec_fwd[0].file_name, "src/pkg.1111aaaa.js");
    assert_eq!(rec_fwd[1].file_name, "src/PKG.JS/child.js");

    let reverse = vec![make_module("src/PKG.JS/child.js", "3333cccc4444dddd", 0, source.len()), make_module("src/pkg.js", "1111aaaa2222bbbb", 0, source.len())];
    let rec_rev = recover_modules(source, &reverse, RewriteLevel::None, &Limits::default());
    let dir_rev = TempDir::new("file-dir-rev");
    write_all_exclusively(dir_rev.path(), &rec_rev);
    assert_eq!(rec_rev[0].file_name, "src/PKG.JS/child.js");
    assert_eq!(rec_rev[1].file_name, "src/pkg.1111aaaa.js");
}

/// Non-colliding modules preserve their original mixed-case spelling untouched.
#[test]
fn distinct_modules_keep_original_casing_control() {
    let source = "globalThis.keep = 1;";
    let modules = vec![
        make_module("src/Alpha.js", "1111222233334444", 0, source.len()),
        make_module("src/Beta.ts", "5555666677778888", 0, source.len()),
        make_module("src/Gamma.mjs", "9999aaaabbbbcccc", 0, source.len()),
    ];

    let recovered = recover_modules(source, &modules, RewriteLevel::None, &Limits::default());
    let names: Vec<&str> = recovered.iter().map(|m| m.file_name.as_str()).collect();
    assert_eq!(names, vec!["src/Alpha.js", "src/Beta.ts", "src/Gamma.mjs"]);
}
