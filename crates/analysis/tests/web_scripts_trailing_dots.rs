//! Regression tests for trailing-dot host and path segment normalization in
//! `web_scripts::plan_layout`.

use craft_anything_analysis::web_scripts::{CapturedScript, PlannedFile, plan_layout};
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
        let dir = std::env::temp_dir().join(format!("craft-web-trailing-dots-{}-{}-{label}", std::process::id(), id));
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
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("failed creating parent for {:?} ({}): {e}", s.url, f.path));
        }
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("failed exclusive write for {:?} at {}: {e}", s.url, f.path));
        out.write_all(s.text.as_bytes()).expect("write script bytes");
    }
}

/// FQDN trailing-dot hosts, trailing-dot path segments, and 100-char segment cuts landing on `.` strip trailing dots so Windows writes never collide.
#[test]
fn fqdn_trailing_dot_hosts_and_trailing_dot_segments_write_exclusively() {
    let p99 = "a".repeat(99);
    let cut_dot_url = format!("https://ex.test/{p99}.b/app.js");
    let plain_99_url = format!("https://ex.test/{p99}/app.js");

    let scripts = vec![
        CapturedScript { url: "https://ex.test./assets/app.js".into(), text: "s0();\n".into() },
        CapturedScript { url: "https://ex.test/assets/app.js".into(), text: "s1();\n".into() },
        CapturedScript { url: "https://ex.test/pkg./bundle.js".into(), text: "s2();\n".into() },
        CapturedScript { url: "https://ex.test/pkg/bundle.js".into(), text: "s3();\n".into() },
        CapturedScript { url: "https://ex.test/dir.js./child.js".into(), text: "s4();\n".into() },
        CapturedScript { url: "https://ex.test/dir.js".into(), text: "s5();\n".into() },
        CapturedScript { url: "https://ex.test/leaf.js.".into(), text: "s6();\n".into() },
        CapturedScript { url: "https://ex.test/leaf.js".into(), text: "s7();\n".into() },
        CapturedScript { url: cut_dot_url, text: "s8();\n".into() },
        CapturedScript { url: plain_99_url, text: "s9();\n".into() },
        CapturedScript { url: "https://ex.test/.../dots.js".into(), text: "s10();\n".into() },
    ];

    let plan = plan_layout(&scripts);
    assert_eq!(plan.len(), scripts.len());

    let dir = TempDir::new("fqdn-and-segments");
    write_plan_exclusively(dir.path(), &scripts, &plan);

    let h1 = &sha256_hex(b"s1();\n")[..10];
    let h3 = &sha256_hex(b"s3();\n")[..10];
    let h5 = &sha256_hex(b"s5();\n")[..10];
    let h7 = &sha256_hex(b"s7();\n")[..10];
    let h9 = &sha256_hex(b"s9();\n")[..10];

    assert_eq!(plan[0].path, "files/ex.test/assets/app.js");
    assert_eq!(plan[1].path, format!("files/ex.test/assets/app.{h1}.js"));
    assert_eq!(plan[2].path, "files/ex.test/pkg/bundle.js");
    assert_eq!(plan[3].path, format!("files/ex.test/pkg/bundle.{h3}.js"));
    assert_eq!(plan[4].path, "files/ex.test/dir.js/child.js");
    assert_eq!(plan[5].path, format!("files/ex.test/dir.{h5}.js"));
    assert_eq!(plan[6].path, "files/ex.test/leaf.js");
    assert_eq!(plan[7].path, format!("files/ex.test/leaf.{h7}.js"));
    assert_eq!(plan[8].path, format!("files/ex.test/{p99}/app.js"));
    assert_eq!(plan[9].path, format!("files/ex.test/{p99}/app.{h9}.js"));
    assert_eq!(plan[10].path, "files/ex.test/_/dots.js");

    let keys: HashSet<String> = plan.iter().map(|p| module_path_key(&p.path)).collect();
    assert_eq!(keys.len(), scripts.len());

    let rev_scripts = vec![
        CapturedScript { url: "https://ex.test/dir.js".into(), text: "s5();\n".into() },
        CapturedScript { url: "https://ex.test/dir.js./child.js".into(), text: "s4();\n".into() },
    ];
    let rev_plan = plan_layout(&rev_scripts);
    let dir_rev = TempDir::new("fqdn-and-segments-rev");
    write_plan_exclusively(dir_rev.path(), &rev_scripts, &rev_plan);
    assert_eq!(rev_plan[0].path, format!("files/ex.test/dir.{h5}.js"));
    assert_eq!(rev_plan[1].path, "files/ex.test/dir.js/child.js");
}

/// Ordinary hosts with ports, non-empty leading-dot segments, and internal version dots keep expected layout paths.
#[test]
fn ordinary_hosts_and_internal_dots_control() {
    let scripts = vec![
        CapturedScript { url: "https://cdn.ex.test:8443/v1.2/app.min.js".into(), text: "ok();\n".into() },
        CapturedScript { url: "https://.ex.test/.well-known/app.js".into(), text: "wk();\n".into() },
    ];
    let plan = plan_layout(&scripts);
    assert_eq!(plan[0].path, "files/cdn.ex.test_8443/v1.2/app.min.js");
    assert_eq!(plan[0].reason, None);
    assert_eq!(plan[1].path, "files/ex.test/well-known/app.js");
}
