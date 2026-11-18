//! Adapters for optional external engines (Ghidra, adb, Chromium, Frida, JADX…).
//!
//! Every engine is an entry in [`ENGINES`] (see [`table`]). Each is found, in order: the path the
//! user set in Settings → Engines, the managed install in the engine store, `PATH`, then the
//! engine's standard install folders. Nothing here is required for Clean room analysis.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod adb;
pub mod assets;
pub mod cdp;
pub mod cutter;
pub mod frida;
pub mod game;
pub mod gdb;
pub mod ghidra;
pub mod install;
pub mod jadx;
#[cfg(test)]
mod known_roots_tests;
pub mod mcp_client;
pub mod rizin;
pub mod table;
#[cfg(feature = "wasm-plugins")]
pub mod wasm_plugin;

pub use table::{ArchiveFormat, ENGINES, ExternalEngine, Install, Pin};

use std::path::{Path, PathBuf};

/// Cut `line` to at most `max` bytes without splitting a UTF-8 character. The engine parsers cap
/// each line before inspecting hostile output; a plain byte slice can land inside a character, and
/// keeping the whole line then would defeat the cap.
pub(crate) fn capped_line(line: &str, max: usize) -> &str {
    let mut end = max.min(line.len());
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    line.get(..end).unwrap_or_default()
}

/// User overrides, mirrored from the engine settings so this crate doesn't depend on the engine.
pub trait ToolPathLookup {
    fn override_for(&self, id: &str) -> Option<PathBuf>;
}

/// The table entry for an engine id.
pub fn engine(id: &str) -> Option<&'static ExternalEngine> {
    ENGINES.iter().find(|e| e.id == id)
}

/// The ids that have a path override in Settings → Engines, in table order.
pub fn path_setting_ids() -> impl Iterator<Item = &'static str> {
    ENGINES.iter().filter(|e| e.path_setting).map(|e| e.id)
}

pub fn display_name(id: &str) -> &'static str {
    engine(id).map(|e| e.name).unwrap_or("an external tool")
}

/// What to tell the person when an engine wasn't found, as (reason, remediation): `set` is the path
/// in Settings → Engines, which wasn't there or was a folder without the program, and `flatpak`
/// whether this app runs in that sandbox.
pub fn not_found_help(id: &str, set: Option<&Path>, flatpak: bool) -> (String, String) {
    let name = display_name(id);
    // Settings → Engines can only install what has pins; these the person installs themselves.
    let manual = engine(id).is_some_and(|e| matches!(e.install, Install::Manual));
    let program = engine(id).and_then(|e| e.host_exes().first().copied());
    let reason = match set {
        // A folder that's there: what's missing is the program in it.
        Some(p) if p.is_dir() => match program {
            Some(exe) => format!(
                "This needs {name}, and the folder set for it in Settings → Engines ({}) has no {name} program in it (`{exe}`, looked for in the folder, its `bin` folder and the folders inside it).",
                p.display()
            ),
            None => format!("This needs {name}, and the path set for it in Settings → Engines ({}) is a folder, not the program.", p.display()),
        },
        Some(p) => format!("This needs {name}, and the path set for it in Settings → Engines ({}) isn't there.", p.display()),
        None => format!("This needs {name} installed."),
    };
    const CORRECT: &str = "Correct the path in Settings → Engines, or clear it to look in the usual places.";
    let sandbox = (flatpak && manual).then(|| {
        format!(
            "This app is running as a Flatpak, and its sandbox can't see programs in your system's folders, so a path to one won't work. To use {name} from your computer, run the AppImage, .deb, .rpm or tarball version instead."
        )
    });
    let remediation = match (set, sandbox) {
        (Some(_), Some(sandbox)) => format!("{CORRECT} {sandbox}"),
        (Some(_), None) => CORRECT.to_string(),
        (None, Some(sandbox)) => sandbox,
        (None, None) if manual => format!("Install {name}, then set its path in Settings → Engines if it still isn't found."),
        (None, None) => format!("Install {name} from Settings → Engines."),
    };
    (reason, remediation)
}

/// Find an engine: user override, then the managed store (its [`ExternalEngine::store_folder`],
/// `<store>/<id>` unless a guided setup puts it elsewhere), then PATH,
/// then its standard install folders, then the folders where an unpacked copy is usually kept.
pub fn locate(id: &str, overrides: &dyn ToolPathLookup, store: &Path) -> Option<PathBuf> {
    locate_configured(id, overrides, store).or_else(|| {
        let e = engine(id)?;
        find_on_path(e.host_exes())
            .or_else(|| e.known_install_paths().into_iter().find(|p| p.is_file()))
            .or_else(|| find_in_known_roots(id, e.host_exes(), &e.known_root_folders()))
    })
}

/// Find an engine where the settings or the managed store say only: what [`locate`] finds before
/// it looks around the computer (PATH and standard install folders).
pub fn locate_configured(id: &str, overrides: &dyn ToolPathLookup, store: &Path) -> Option<PathBuf> {
    if let Some(p) = overrides.override_for(id).and_then(|p| in_path_setting(id, &p)) {
        return Some(p);
    }
    let e = engine(id)?;
    let exes = e.host_exes();
    let managed = store.join(e.store_folder());
    if let Some(p) = in_managed(&managed, exes) {
        return Some(p);
    }
    // A single-file install (a .jar) is found by the name its pin saved it under.
    for pin in e.pins() {
        if let ArchiveFormat::File { name } = pin.format
            && managed.join(name).is_file()
        {
            return Some(managed.join(name));
        }
    }
    None
}

/// The program a path set in Settings → Engines names: the file itself, or, for a folder (a
/// JDK's folder, an unzipped JADX), the engine's program inside it as an install folder holds it
/// (see [`in_managed`]), or its single-file install (`vineflower.jar`). `None` when there's no
/// such program, so a folder is never run as the program.
fn in_path_setting(id: &str, p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    if !p.is_dir() {
        return None;
    }
    let e = engine(id)?;
    in_managed(p, e.host_exes()).or_else(|| {
        e.pins().iter().find_map(|pin| match pin.format {
            ArchiveFormat::File { name } => Some(p.join(name)).filter(|f| f.is_file()),
            _ => None,
        })
    })
}

/// An executable in a managed install folder: `<dir>/<exe>`, `<dir>/bin/<exe>` or
/// `<dir>/support/<exe>`, in `dir` itself or in one of its sub-folders, newest version first (an
/// archive unpacks into one top folder, often versioned: `ghidra_12.1.4_PUBLIC/support/…`; a
/// folder of JDKs holds `java-8-…` beside `java-21-…`).
fn in_managed(dir: &Path, exes: &[&str]) -> Option<PathBuf> {
    let here = |d: &Path| exes.iter().flat_map(|exe| [d.join(exe), d.join("bin").join(exe), d.join("support").join(exe)]).find(|p| p.is_file());
    if let Some(p) = here(dir) {
        return Some(p);
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subs.sort_by(|a, b| version_order(&a.file_name().unwrap_or_default().to_string_lossy(), &b.file_name().unwrap_or_default().to_string_lossy()));
    subs.iter().rev().find_map(|d| here(d))
}

/// An unpacked install in one of `roots` (each with how many levels down to look): a folder whose
/// name starts with the engine id (`ghidra`, `Ghidra`, `ghidra_12.1.4_PUBLIC`) and holds one of
/// `exes` (see [`in_managed`]). Newer names first. Reads folder names only, never file contents.
pub fn find_in_known_roots(id: &str, exes: &[&str], roots: &[(PathBuf, usize)]) -> Option<PathBuf> {
    roots.iter().find_map(|(root, depth)| search_named(root, id, exes, *depth))
}

/// Folder names in version order: runs of digits compare as numbers (`java-8` before `java-17`,
/// `12.1.4` before `12.10.0`), the rest as text.
fn version_order(a: &str, b: &str) -> std::cmp::Ordering {
    fn parts(s: &str) -> Vec<(bool, &str)> {
        let mut out = Vec::new();
        let mut start = 0;
        let mut digit = None;
        for (i, c) in s.char_indices() {
            let d = c.is_ascii_digit();
            if digit.is_some_and(|was| was != d) {
                out.push((digit == Some(true), &s[start..i]));
                start = i;
            }
            digit = Some(d);
        }
        if start < s.len() {
            out.push((digit == Some(true), &s[start..]));
        }
        out
    }
    let (pa, pb) = (parts(a), parts(b));
    for ((da, sa), (db, sb)) in pa.iter().zip(&pb) {
        let o = if *da && *db {
            let (ta, tb) = (sa.trim_start_matches('0'), sb.trim_start_matches('0'));
            ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb))
        } else {
            sa.cmp(sb)
        };
        if o.is_ne() {
            return o;
        }
    }
    pa.len().cmp(&pb.len()).then_with(|| a.cmp(b))
}

fn search_named(dir: &Path, id: &str, exes: &[&str], depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    let mut subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    subs.sort_by(|a, b| version_order(&a.file_name().unwrap_or_default().to_string_lossy(), &b.file_name().unwrap_or_default().to_string_lossy()));
    subs.iter().rev().find_map(|d| {
        let named = d.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_ascii_lowercase().starts_with(id));
        if named && let Some(p) = in_managed(d, exes) {
            return Some(p);
        }
        search_named(d, id, exes, depth - 1)
    })
}

/// Standard install locations for programs that don't put themselves on PATH (browsers on
/// Windows and macOS, Ghidra from Linux packages), most common first.
pub fn well_known(id: &str) -> Vec<PathBuf> {
    engine(id).map(ExternalEngine::known_install_paths).unwrap_or_default()
}

pub(crate) fn find_on_path(exes: &[&str]) -> Option<PathBuf> {
    let path = craft_anything_platform::which::search_path()?;
    for dir in std::env::split_paths(&path) {
        for exe in exes {
            let p = dir.join(exe);
            if p.is_file() {
                return Some(p);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoOverrides;
    impl ToolPathLookup for NoOverrides {
        fn override_for(&self, _: &str) -> Option<PathBuf> {
            None
        }
    }

    #[test]
    fn unknown_engine_is_none() {
        assert!(locate("definitely-not-an-engine", &NoOverrides, Path::new("/nonexistent")).is_none());
        assert_eq!(display_name("adb"), "Android platform tools (adb)");
    }

    /// Asked to install Java, we say Ghidra's setup brings it; that setup unpacks it under
    /// `<store>/jdk`, which the Java runtime engine has to find (it was looked for in `<store>/java`).
    #[test]
    fn the_java_ghidras_setup_brings_is_the_java_runtime() {
        let java = if cfg!(windows) { "java.exe" } else { "java" };
        // Linux and Windows keep the JDK's own top folder; macOS's pin strips it down to `Home`.
        for top in ["jdk-21.0.12.1+1", "Home"] {
            let store = tempfile::tempdir().unwrap();
            let exe = store.path().join("jdk").join(top).join("bin").join(java);
            std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
            std::fs::write(&exe, "").unwrap();
            assert_eq!(locate_configured("java", &NoOverrides, store.path()), Some(exe.clone()), "{top}");
            // The same Java Ghidra is pointed at.
            assert_eq!(ghidra::get::java_home(None, store.path()).map(|h| h.join("bin").join(java)), Some(exe));
        }
    }

    /// Ghidra's zip unpacks into `<store>/ghidra/ghidra_<version>_PUBLIC/`, with the launcher in
    /// `support/`; `engines_list` said "not found" for an install that Deep mode was using.
    #[test]
    fn a_managed_install_in_a_versioned_folder_is_found() {
        let store = tempfile::tempdir().unwrap();
        let exe = if cfg!(windows) { "analyzeHeadless.bat" } else { "analyzeHeadless" };
        let launcher = store.path().join("ghidra").join("ghidra_12.1.4_PUBLIC").join("support").join(exe);
        std::fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        std::fs::write(&launcher, b"x").unwrap();
        assert_eq!(locate("ghidra", &NoOverrides, store.path()), Some(launcher));
        // An older unpack beside it loses to the newest name.
        let newer = store.path().join("ghidra").join("ghidra_12.2.0_PUBLIC").join("support").join(exe);
        std::fs::create_dir_all(newer.parent().unwrap()).unwrap();
        std::fs::write(&newer, b"x").unwrap();
        assert_eq!(locate("ghidra", &NoOverrides, store.path()), Some(newer));
        // Nothing but folders: still none, not a panic.
        let empty = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(empty.path().join("ghidra").join("ghidra_12.1.4_PUBLIC")).unwrap();
        // (Unless this computer has Ghidra on PATH or from a system package.)
        let installed = find_on_path(&[exe]).is_some() || well_known("ghidra").iter().any(|p| p.is_file());
        assert!(locate("ghidra", &NoOverrides, empty.path()).is_none() || installed);
    }

    struct SetPath(PathBuf);
    impl ToolPathLookup for SetPath {
        fn override_for(&self, _: &str) -> Option<PathBuf> {
            Some(self.0.clone())
        }
    }

    /// A path set to a folder (a JDK's folder for the Java runtime) counted as found, and running
    /// the folder then failed with "permission denied". The program in it is found instead, as an
    /// install folder holds it; a folder without it isn't found, and says so.
    #[test]
    fn a_folder_set_as_an_engines_path_is_looked_in() {
        let java = if cfg!(windows) { "java.exe" } else { "java" };
        let store = tempfile::tempdir().unwrap();
        let d = tempfile::tempdir().unwrap();
        let jdk = d.path().join("jdk-21");
        let exe = jdk.join("bin").join(java);
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, "").unwrap();
        // The JDK's folder, or the folder that holds JDKs (`/usr/lib/jvm`).
        assert_eq!(locate_configured("java", &SetPath(jdk.clone()), store.path()), Some(exe.clone()));
        assert_eq!(locate_configured("java", &SetPath(d.path().to_path_buf()), store.path()), Some(exe.clone()));
        // The program itself is taken as it is, as before.
        assert_eq!(locate_configured("java", &SetPath(exe.clone()), store.path()), Some(exe));
        // A single-file install's folder: its jar.
        let jar = d.path().join("vf").join("vineflower.jar");
        std::fs::create_dir_all(jar.parent().unwrap()).unwrap();
        std::fs::write(&jar, "").unwrap();
        assert_eq!(locate_configured("vineflower", &SetPath(jar.parent().unwrap().to_path_buf()), store.path()), Some(jar.clone()));
        assert_eq!(locate_configured("vineflower", &SetPath(jar.clone()), store.path()), Some(jar));
        // A folder without the program isn't found (the empty store holds nothing either)...
        let empty = d.path().join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        assert_eq!(locate_configured("java", &SetPath(empty.clone()), store.path()), None);
        // ...and the message says what's missing, not that the folder isn't there.
        let (reason, fix) = not_found_help("java", Some(&empty), false);
        assert!(reason.contains("has no Java") && reason.contains(&format!("`{java}`")) && !reason.contains("isn't there"), "{reason}");
        assert!(fix.starts_with("Correct the path"), "{fix}");
    }

    /// A folder of JDKs (`/usr/lib/jvm`) gives its newest one, not the last by plain text order
    /// (`java-8` sorts after `java-21` as text).
    #[test]
    fn a_folder_of_versions_gives_the_newest() {
        let java = if cfg!(windows) { "java.exe" } else { "java" };
        let store = tempfile::tempdir().unwrap();
        let d = tempfile::tempdir().unwrap();
        for v in ["java-8-openjdk", "java-17-openjdk", "java-21-openjdk"] {
            std::fs::create_dir_all(d.path().join(v).join("bin")).unwrap();
            std::fs::write(d.path().join(v).join("bin").join(java), "").unwrap();
        }
        let want = d.path().join("java-21-openjdk").join("bin").join(java);
        assert_eq!(locate_configured("java", &SetPath(d.path().to_path_buf()), store.path()), Some(want));
        use std::cmp::Ordering::*;
        assert_eq!(version_order("ghidra_12.1.4_PUBLIC", "ghidra_12.10.0_PUBLIC"), Less);
        assert_eq!(version_order("ghidra_12.2.0_PUBLIC", "ghidra_12.1.4_PUBLIC"), Greater);
        assert_eq!(version_order("jdk-021", "jdk-21"), Less);
        assert_eq!(version_order("a", "a"), Equal);
    }

    /// #841: a path set for a browser was ignored without a word, and the message offered an
    /// install that Settings can't do.
    #[test]
    fn a_missing_engine_is_explained_by_why_it_was_not_found() {
        let set = Path::new("/usr/bin/brave");
        let (reason, fix) = not_found_help("chromium", Some(set), true);
        assert!(reason.contains(&set.display().to_string()), "{reason}");
        // The path may only be mistyped, and one in the home folder can work, so correcting it comes first.
        assert!(fix.starts_with("Correct the path") && fix.contains("Flatpak") && fix.contains("AppImage"), "{fix}");
        // A Flatpak can't reach a browser with no path set either, and there's no path to correct.
        let (_, fix) = not_found_help("chromium", None, true);
        assert!(fix.contains("Flatpak") && !fix.contains("Correct the path"), "{fix}");

        let (reason, fix) = not_found_help("chromium", Some(set), false);
        assert!(reason.contains("isn't there") && fix.contains("Correct the path") && !fix.contains("Flatpak"), "{reason} {fix}");

        let (_, fix) = not_found_help("chromium", None, false);
        assert!(fix.starts_with("Install a Chromium-based browser,") && !fix.contains("from Settings"), "{fix}");

        // What Settings can install still says so, and a sandbox doesn't stop it.
        for flatpak in [false, true] {
            assert_eq!(not_found_help("adb", None, flatpak).1, "Install Android platform tools (adb) from Settings → Engines.");
        }
    }

    #[test]
    fn browsers_are_looked_for_under_their_linux_package_names() {
        let exes = engine("chromium").unwrap().exes;
        assert!(["google-chrome-stable", "brave-browser", "microsoft-edge"].iter().all(|x| exes.contains(x)), "{exes:?}");
    }

    #[test]
    fn browsers_have_well_known_locations_off_path() {
        assert!(well_known("adb").is_empty());
        let paths = well_known("chromium");
        if cfg!(any(windows, target_os = "macos")) {
            assert!(paths.iter().any(|p| p.to_string_lossy().contains("Chrome")));
            assert!(paths.iter().any(|p| p.to_string_lossy().contains("Edge")));
        }
        // #85: Linux packages put Ghidra in its own folder with only a start script on PATH.
        let ghidra = well_known("ghidra");
        if cfg!(target_os = "linux") {
            assert_eq!(ghidra, [PathBuf::from("/opt/ghidra/support/analyzeHeadless"), PathBuf::from("/usr/share/ghidra/support/analyzeHeadless")]);
        } else {
            assert!(ghidra.is_empty());
        }
    }
}
