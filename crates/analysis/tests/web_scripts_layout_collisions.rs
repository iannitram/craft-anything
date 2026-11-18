//! Regression tests for `web_scripts::plan_layout` case collisions, uppercase script extensions,
//! and repeated identical script suffix bounds.

use craft_anything_analysis::web_scripts::{CapturedScript, PlannedFile, plan_layout, scripts_from_har};
use craft_anything_evidence::sha256_hex;
use craft_anything_js::recover::module_path_key;
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
        let dir = std::env::temp_dir().join(format!("craft-web-scripts-{}-{}-{label}", std::process::id(), id));
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

/// Write every planned script exclusively (`create_new(true)`) into `root`, mirroring `web_scripts_save`.
fn write_plan_exclusively(root: &Path, scripts: &[CapturedScript], plan: &[PlannedFile]) {
    assert_eq!(scripts.len(), plan.len());
    for (s, f) in scripts.iter().zip(plan.iter()) {
        let mut path = root.to_path_buf();
        for comp in f.path.split('/') {
            path.push(comp);
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("failed creating parent for {}: {e}", f.path));
        }
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("failed exclusive write for {:?} at {}: {e}", s.url, f.path));
        out.write_all(s.text.as_bytes()).expect("write script bytes");
    }
}

/// Case-only differences across hosts, paths, generated hash suffixes, and Unicode letters (including final Greek sigma) never collide on disk.
#[test]
fn case_only_hosts_paths_and_hash_suffixes_write_exclusively() {
    let dup_body = "console.log('dup');\n";
    let dup_short = sha256_hex(dup_body.as_bytes())[..10].to_string();
    let upper_short = dup_short.to_ascii_uppercase();

    let scripts = vec![
        CapturedScript { url: "https://CDN.ex.test/Assets/App.js".into(), text: "a();\n".into() },
        CapturedScript { url: "https://cdn.ex.test/assets/app.js".into(), text: "b();\n".into() },
        CapturedScript { url: format!("https://ex.test/bundle.{upper_short}.js"), text: "c();\n".into() },
        CapturedScript { url: "https://ex.test/Bundle.js".into(), text: "d();\n".into() },
        CapturedScript { url: "https://ex.test/bundle.js".into(), text: dup_body.into() },
        CapturedScript { url: "https://ex.test/Über.js".into(), text: "u1();\n".into() },
        CapturedScript { url: "https://ex.test/über.js".into(), text: "u2();\n".into() },
        CapturedScript { url: "https://ex.test/ΟΣ.js".into(), text: "g1();\n".into() },
        CapturedScript { url: "https://ex.test/οσ.js".into(), text: "g2();\n".into() },
        CapturedScript { url: "https://ex.test/ος.js".into(), text: "g3();\n".into() },
        CapturedScript { url: "https://ex.test/\u{212a}.js".into(), text: "k1();\n".into() },
        CapturedScript { url: "https://ex.test/K.js".into(), text: "k2();\n".into() },
        CapturedScript { url: "https://ex.test/\u{212b}.js".into(), text: "a1();\n".into() },
        CapturedScript { url: "https://ex.test/\u{00c5}.js".into(), text: "a2();\n".into() },
        CapturedScript { url: "https://ex.test/straße.js".into(), text: "s1();\n".into() },
        CapturedScript { url: "https://ex.test/STRASSE.js".into(), text: "s2();\n".into() },
        CapturedScript { url: "https://ex.test/\u{ac00}.js".into(), text: "h1();\n".into() },
        CapturedScript { url: "https://ex.test/\u{1100}\u{1161}.js".into(), text: "h2();\n".into() },
        CapturedScript { url: "https://ex.test/\u{d7a3}.js".into(), text: "h3();\n".into() },
        CapturedScript { url: "https://ex.test/\u{1112}\u{1175}\u{11c2}.js".into(), text: "h4();\n".into() },
        CapturedScript { url: "https://ex.test/\u{0131}.js".into(), text: "i3();\n".into() },
        CapturedScript { url: "https://ex.test/I.js".into(), text: "i4();\n".into() },
    ];

    let plan = plan_layout(&scripts);
    assert_eq!(plan.len(), scripts.len());

    let dir = TempDir::new("case-collisions");
    write_plan_exclusively(dir.path(), &scripts, &plan);

    let k2_short = sha256_hex(b"k2();\n")[..10].to_string();
    let s2_short = sha256_hex(b"s2();\n")[..10].to_string();
    let h2_short = sha256_hex(b"h2();\n")[..10].to_string();
    let h4_short = sha256_hex(b"h4();\n")[..10].to_string();
    assert_eq!(plan[0].path, "files/CDN.ex.test/Assets/App.js");
    assert_eq!(plan[2].path, format!("files/ex.test/bundle.{upper_short}.js"));
    assert_eq!(plan[3].path, "files/ex.test/Bundle.js");
    assert_eq!(plan[4].path, format!("files/ex.test/bundle.{dup_short}.2.js"));
    assert_eq!(plan[10].path, "files/ex.test/\u{212a}.js");
    assert_eq!(plan[11].path, format!("files/ex.test/K.{k2_short}.js"));
    assert_eq!(plan[14].path, "files/ex.test/straße.js");
    assert_eq!(plan[15].path, format!("files/ex.test/STRASSE.{s2_short}.js"));
    assert_eq!(plan[16].path, "files/ex.test/\u{ac00}.js");
    assert_eq!(plan[17].path, format!("files/ex.test/\u{1100}\u{1161}.{h2_short}.js"));
    assert_eq!(plan[18].path, "files/ex.test/\u{d7a3}.js");
    assert_eq!(plan[19].path, format!("files/ex.test/\u{1112}\u{1175}\u{11c2}.{h4_short}.js"));

    let shared_keys: HashSet<String> = plan.iter().map(|p| module_path_key(&p.path)).collect();
    assert_eq!(shared_keys.len(), scripts.len(), "case-folded paths must be unique: {plan:?}");
}

/// File/parent-directory collisions (`pkg.JS` vs `pkg.JS/child.js`) resolve in either input order while unconflicted `.JS` paths stay unchanged.
#[test]
fn file_and_parent_directory_conflicts_resolve_in_either_order() {
    let p_short = sha256_hex(b"parent();\n")[..10].to_string();

    let forward = vec![
        CapturedScript { url: "https://ex.test/pkg.JS".into(), text: "parent();\n".into() },
        CapturedScript { url: "https://ex.test/pkg.JS/child.js".into(), text: "child();\n".into() },
        CapturedScript { url: "https://ex.test/solo.JS".into(), text: "solo();\n".into() },
    ];
    let plan_fwd = plan_layout(&forward);
    let dir_fwd = TempDir::new("file-dir-fwd");
    write_plan_exclusively(dir_fwd.path(), &forward, &plan_fwd);
    assert_eq!(plan_fwd[0].path, format!("files/ex.test/pkg.{p_short}.JS"));
    assert_eq!(plan_fwd[1].path, "files/ex.test/pkg.JS/child.js");
    assert_eq!(plan_fwd[2].path, "files/ex.test/solo.JS");

    let reverse = vec![
        CapturedScript { url: "https://ex.test/pkg.JS/child.js".into(), text: "child();\n".into() },
        CapturedScript { url: "https://ex.test/pkg.JS".into(), text: "parent();\n".into() },
        CapturedScript { url: "https://ex.test/solo.JS".into(), text: "solo();\n".into() },
    ];
    let plan_rev = plan_layout(&reverse);
    let dir_rev = TempDir::new("file-dir-rev");
    write_plan_exclusively(dir_rev.path(), &reverse, &plan_rev);
    assert_eq!(plan_rev[0].path, "files/ex.test/pkg.JS/child.js");
    assert_eq!(plan_rev[1].path, format!("files/ex.test/pkg.{p_short}.JS"));
    assert_eq!(plan_rev[2].path, "files/ex.test/solo.JS");
}

/// Uppercase and mixed-case `.JS`, `.MJS`, and `.CJS` URLs extracted from HAR files keep their extension without appending `.js`.
#[test]
fn uppercase_and_mixed_case_script_extensions_do_not_double_append_js() {
    let har = br#"{"log":{"entries":[
        {"request":{"method":"GET","url":"https://ex.test/APP.JS"},"response":{"status":200,"content":{"mimeType":"text/plain","text":"a1()"}}},
        {"request":{"method":"GET","url":"https://ex.test/worker.MJS"},"response":{"status":200,"content":{"mimeType":"text/plain","text":"m1()"}}},
        {"request":{"method":"GET","url":"https://ex.test/worker.MJS"},"response":{"status":200,"content":{"mimeType":"text/plain","text":"m2()"}}},
        {"request":{"method":"GET","url":"https://ex.test/common.CJS"},"response":{"status":200,"content":{"mimeType":"text/plain","text":"c1()"}}},
        {"request":{"method":"GET","url":"https://ex.test/api/chunk"},"response":{"status":200,"content":{"mimeType":"application/javascript","text":"k1()"}}}
    ]}}"#;

    let scripts = scripts_from_har(har, 100).expect("parse HAR");
    assert_eq!(scripts.len(), 5);

    let plan = plan_layout(&scripts);
    let dir = TempDir::new("upper-ext");
    write_plan_exclusively(dir.path(), &scripts, &plan);

    let m2_short = sha256_hex(b"m2()")[..10].to_string();
    assert_eq!(plan[0].path, "files/ex.test/APP.JS");
    assert_eq!(plan[1].path, "files/ex.test/worker.MJS");
    assert_eq!(plan[2].path, format!("files/ex.test/worker.{m2_short}.MJS"));
    assert_eq!(plan[3].path, "files/ex.test/common.CJS");
    assert_eq!(plan[4].path, "files/ex.test/api/chunk.js");
}

/// Repeated identical scripts receive flat `.hash.N` suffixes instead of chaining `.2.3.4...` past filesystem filename limits.
#[test]
fn repeated_identical_scripts_keep_bounded_flat_occurrence_suffixes_and_write_to_disk() {
    let body = "window.__poll = 1;\n";
    let short = sha256_hex(body.as_bytes())[..10].to_string();
    let scripts: Vec<CapturedScript> = (0..120).map(|_| CapturedScript { url: "https://ex.test/assets/poll.js".into(), text: body.into() }).collect();

    let plan = plan_layout(&scripts);
    assert_eq!(plan.len(), 120);

    let dir = TempDir::new("repeated-120");
    write_plan_exclusively(dir.path(), &scripts, &plan);

    assert_eq!(plan[0].path, "files/ex.test/assets/poll.js");
    assert_eq!(plan[1].path, format!("files/ex.test/assets/poll.{short}.js"));
    assert_eq!(plan[2].path, format!("files/ex.test/assets/poll.{short}.2.js"));
    assert_eq!(plan[3].path, format!("files/ex.test/assets/poll.{short}.3.js"));
    assert_eq!(plan[119].path, format!("files/ex.test/assets/poll.{short}.119.js"));
    assert!(plan.iter().all(|p| p.path.split('/').all(|seg| seg.len() <= 64)));
}

/// Repeated identical query-string scripts increment `.N` on the hashed name instead of duplicating `.hash.hash`.
#[test]
fn repeated_query_scripts_do_not_duplicate_content_hash_segment() {
    let body = "window.__v = 1;\n";
    let short = sha256_hex(body.as_bytes())[..10].to_string();
    let scripts = vec![
        CapturedScript { url: "https://ex.test/assets/app.js?v=1".into(), text: body.into() },
        CapturedScript { url: "https://ex.test/assets/app.js?v=1".into(), text: body.into() },
        CapturedScript { url: "https://ex.test/assets/app.js?v=1".into(), text: body.into() },
    ];

    let plan = plan_layout(&scripts);
    assert_eq!(plan[0].path, format!("files/ex.test/assets/app.{short}.js"));
    assert_eq!(plan[1].path, format!("files/ex.test/assets/app.{short}.2.js"));
    assert_eq!(plan[2].path, format!("files/ex.test/assets/app.{short}.3.js"));
    assert!(plan[0].reason.as_deref().unwrap_or("").contains("query string"));
    assert!(plan[1].reason.as_deref().unwrap_or("").contains("already used"));
}

/// Distinct scripts, file:// URLs, and inline scripts preserve existing layout paths and reasons.
#[test]
fn ordinary_capture_layout_control() {
    let scripts = vec![
        CapturedScript { url: "https://ex.test/main.js".into(), text: "main()".into() },
        CapturedScript { url: "file:///opt/app/preload.cjs".into(), text: "preload()".into() },
        CapturedScript { url: String::new(), text: "inline()".into() },
    ];

    let plan = plan_layout(&scripts);
    assert_eq!(plan[0].path, "files/ex.test/main.js");
    assert_eq!(plan[0].reason, None);
    assert_eq!(plan[1].path, "files/local/opt/app/preload.cjs");
    assert_eq!(plan[2].path, "files/inline/script-2.js");
}
