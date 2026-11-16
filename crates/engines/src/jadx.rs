//! Running JADX (the Android/Java decompiler, an external program) on one class.

use craft_anything_evidence::ToolError;
use craft_anything_platform::cancel::CancelToken;
use craft_anything_platform::process::{Output, Process};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The Java home a `java` program sits in (`<home>/bin/java`), as JADX's launcher script wants it
/// in `JAVA_HOME`: the launcher runs `$JAVA_HOME/bin/java`, so a home is given only when that
/// file is there. `None` for a `java` outside a `bin` folder, or one named otherwise (`java17`).
pub fn java_home_of(java: &Path) -> Option<PathBuf> {
    let bin = java.parent().filter(|b| b.file_name().is_some_and(|n| n == "bin"))?;
    let exe = if cfg!(windows) { "java.exe" } else { "java" };
    bin.join(exe).is_file().then(|| bin.parent().map(Path::to_path_buf)).flatten()
}

/// Decompile one class (`class_java`, dotted, `$` for inner classes) of `input` (an APK, DEX or
/// JAR) into `out`, resources skipped. The caller finds `<Outer>.java` under `out`. `java` is
/// the Java runtime engine to run it on; without one, JADX's launcher finds Java itself
/// (`JAVA_HOME`, then PATH).
pub fn run_single_class(jadx: &Path, java: Option<&Path>, input: &Path, class_java: &str, out: &Path, cancel: CancelToken) -> Result<Output, ToolError> {
    let mut process = Process::new(jadx);
    if let Some(home) = java.and_then(java_home_of) {
        process = process.env("JAVA_HOME", home);
    }
    process
        .args(["--no-res", "--quiet", "--single-class", class_java, "-d"])
        .arg(out)
        .arg(input)
        .timeout(Duration::from_secs(180))
        .idle_timeout(Duration::from_secs(120))
        .stdout_cap(1 << 20)
        .stderr_cap(1 << 20)
        .cancel_when(move || cancel.is_cancelled())
        .label("jadx")
        .run()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_home_is_given_only_where_its_launcher_finds_java() {
        let d = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) { "java.exe" } else { "java" };
        let home = d.path().join("jdk-21");
        std::fs::create_dir_all(home.join("bin")).unwrap();
        std::fs::write(home.join("bin").join(exe), "").unwrap();
        assert_eq!(java_home_of(&home.join("bin").join(exe)), Some(home.clone()));
        // A Java under another name with no `java` beside it: the launcher wouldn't find it.
        let other = d.path().join("other").join("bin");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("java17"), "").unwrap();
        assert_eq!(java_home_of(&other.join("java17")), None);
        // Not in a `bin` folder.
        std::fs::write(d.path().join(exe), "").unwrap();
        assert_eq!(java_home_of(&d.path().join(exe)), None);
    }
}
