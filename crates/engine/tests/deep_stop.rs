//! A test program of its own: stopping analyses is process-wide.

use craft_anything_engine::tools::deep;
use craft_anything_engines::ghidra;
use std::time::{Duration, Instant};

/// #946: closing the app ends an analysis, here one waiting for another's lock, within moments.
#[test]
fn closing_stops_an_analysis() {
    let d = tempfile::Builder::new().prefix("ca").tempdir().unwrap();
    let launcher = d.path().join("analyzeHeadless");
    std::fs::write(&launcher, "").unwrap();
    let store = d.path().join("store");
    let mut settings = craft_anything_engine::testing::session().settings.clone();
    settings.engines.store_dir = Some(store.clone());
    settings.engines.ghidra.install_dir = Some(launcher);
    settings.engines.ghidra.startup_timeout_secs = Some(120);
    let bytes = b"not really a program";
    let sha = craft_anything_evidence::sha256_hex(bytes);
    let digest = craft_anything_evidence::sha256_hex(b"craft-anything-ghidra-fast-profile-v2|lang=");
    let project = ghidra::project_cache_dir(&store, &sha, &digest);
    std::fs::create_dir_all(project.parent().unwrap()).unwrap();
    std::fs::write(project.with_extension("analysing"), "").unwrap();

    let waiting = {
        let sha = sha.clone();
        std::thread::spawn(move || deep::prepare(&settings, bytes, &sha, &|_| {}, &|| false))
    };
    let until = Instant::now() + Duration::from_secs(30);
    while !waiting.is_finished() {
        assert!(Instant::now() < until, "the analysis kept going");
        deep::stop_analyses(Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(waiting.join().unwrap().is_err(), "it stopped, it didn't finish");
}
