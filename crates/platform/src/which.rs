//! Find programs on `PATH` (with `PATHEXT` on Windows). No subprocess is run.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The first match for `name` on `PATH`, or `name` itself when it is already a path to a file.
pub fn which(name: &str) -> Option<PathBuf> {
    which_all(name).into_iter().next()
}

/// Every match for `name` on [`search_path`], in order (useful to spot duplicate installs).
pub fn which_all(name: &str) -> Vec<PathBuf> {
    which_in(name, search_path())
}

/// Folders, under the home folder and then system-wide, where coding agents, npm, Homebrew and
/// other installers put command-line programs. OpenCode's installer uses `~/.opencode/bin`, or
/// `~/bin` when there is one (#112). Grok Build's installer uses `~/.grok/bin`.
#[cfg(unix)]
const USUAL_HOME_DIRS: &[&str] =
    &[".local/bin", ".claude/local", ".npm-global/bin", ".bun/bin", ".volta/bin", ".cargo/bin", ".opencode/bin", ".grok/bin", "bin"];
#[cfg(unix)]
const USUAL_DIRS: &[&str] = &["/opt/homebrew/bin", "/usr/local/bin"];
// Claude Code's installer, npm (without a custom prefix), WinGet's portable packages (Claude
// Code's WinGet install), bun, cargo, Hermes Agent's installer (`%LOCALAPPDATA%\hermes\bin`, #116)
// and Grok Build (`%USERPROFILE%\.grok\bin`). PATH may not have them yet in an app started before
// the install.
#[cfg(windows)]
const USUAL_HOME_DIRS: &[&str] =
    &[r".local\bin", r"AppData\Roaming\npm", r"AppData\Local\Microsoft\WinGet\Links", r".bun\bin", r".cargo\bin", r"AppData\Local\hermes\bin", r".grok\bin"];
#[cfg(not(any(unix, windows)))]
const USUAL_HOME_DIRS: &[&str] = &[];
// WinGet's portable packages installed for every user (`--scope machine`).
#[cfg(windows)]
const USUAL_DIRS: &[&str] = &[r"C:\Program Files\WinGet\Links"];
#[cfg(not(any(unix, windows)))]
const USUAL_DIRS: &[&str] = &[];
const HOME_VAR: &str = if cfg!(windows) { "USERPROFILE" } else { "HOME" };

/// `PATH` plus the usual folders for command-line programs, after it so the person's own order
/// wins. An app opened from the Dock or a desktop launcher gets the system's short `PATH`, not the
/// shell's, and would miss a coding agent installed in `~/.local/bin`; on Windows an installer
/// may not have added its folder to `PATH` at all.
pub fn search_path() -> Option<OsString> {
    let local_data = cfg!(windows).then(|| std::env::var_os("LOCALAPPDATA")).flatten().map(PathBuf::from);
    with_usual_dirs(std::env::var_os("PATH"), std::env::var_os(HOME_VAR).map(PathBuf::from), local_data)
}

/// How to start `program`, and the arguments that go before the caller's. On Windows only `.exe`
/// files start by bare name, and npm installs Gemini CLI, Codex and often Claude Code as `.cmd`
/// shims: an npm shim's script is run by its interpreter directly (through `cmd.exe` a prompt with
/// line breaks, or longer than 8191 characters, wouldn't arrive whole), and any other batch file
/// by its full path.
#[cfg(windows)]
pub(crate) fn launch_target(program: &std::ffi::OsStr) -> (OsString, Vec<OsString>) {
    let path = search_path();
    // Started by the path it was found at, not by name: a child with a cleared environment, or a
    // lookup that differs from ours, would say "not found" for a program that is there (#152).
    let Some(found) = which_in(&program.to_string_lossy(), path.clone()).into_iter().next() else { return (program.to_owned(), Vec::new()) };
    match read_shim(&found) {
        Some((dir, None, target)) if is_executable(&dir.join(&target)) => (dir.join(target).into_os_string(), Vec::new()),
        Some((dir, Some(interpreter), target)) if dir.join(&target).is_file() => match find_interpreter(&dir, &interpreter, path) {
            Some(i) => (i.into_os_string(), vec![dir.join(target).into_os_string()]),
            None => (found.into_os_string(), Vec::new()),
        },
        _ => (found.into_os_string(), Vec::new()),
    }
}

#[cfg(not(windows))]
pub(crate) fn launch_target(program: &std::ffi::OsStr) -> (OsString, Vec<OsString>) {
    (program.to_owned(), Vec::new())
}

/// The interpreter (`node`) an npm shim needs when it can't be found: then [`launch_target`] can
/// only start the `.cmd` itself, which can't take an argument with a line break.
#[cfg(windows)]
pub(crate) fn shim_missing_interpreter(program: &std::ffi::OsStr) -> Option<String> {
    let path = search_path();
    let found = which_in(&program.to_string_lossy(), path.clone()).into_iter().next()?;
    missing_interpreter_in(&found, path)
}

#[cfg(not(windows))]
pub(crate) fn shim_missing_interpreter(_program: &std::ffi::OsStr) -> Option<String> {
    None
}

#[cfg(windows)]
fn missing_interpreter_in(shim: &Path, path: Option<OsString>) -> Option<String> {
    let (dir, interpreter, target) = read_shim(shim)?;
    let interpreter = interpreter?;
    (dir.join(&target).is_file() && find_interpreter(&dir, &interpreter, path).is_none()).then_some(interpreter)
}

/// A `.cmd`/`.bat` npm shim's folder, interpreter and target (see [`parse_npm_shim`]).
#[cfg(windows)]
fn read_shim(found: &Path) -> Option<(PathBuf, Option<String>, String)> {
    if !found.extension().is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat")) {
        return None;
    }
    let dir = found.parent().map(Path::to_path_buf).unwrap_or_default();
    let text = std::fs::metadata(found).ok().filter(|m| m.len() <= 64 * 1024).and_then(|_| std::fs::read_to_string(found).ok())?;
    let (interpreter, target) = parse_npm_shim(&text)?;
    Some((dir, interpreter, target))
}

/// The shim's interpreter: `node.exe` beside it, as npm's shim prefers, else on `path`.
#[cfg(windows)]
fn find_interpreter(dir: &Path, interpreter: &str, path: Option<OsString>) -> Option<PathBuf> {
    let local = dir.join(format!("{interpreter}.exe"));
    if local.is_file() { Some(local) } else { which_in(interpreter, path).into_iter().next() }
}

/// What an npm (or pnpm) `.cmd` shim runs: its interpreter (`node`; none when the target is an
/// `.exe`) and the target, relative to the shim's folder. A `.cmd` that only runs one `.exe` by its
/// full path (see [`exe_wrapper_target`]) gives that path, with no interpreter.
#[cfg(any(windows, test))]
fn parse_npm_shim(text: &str) -> Option<(Option<String>, String)> {
    if let Some(exe) = exe_wrapper_target(text) {
        return Some((None, exe));
    }
    let run = text.lines().find(|l| l.contains("%*"))?;
    let quoted: Vec<&str> = run.split('"').skip(1).step_by(2).collect();
    let target = quoted.iter().rev().find_map(|q| q.strip_prefix(r"%dp0%\").or_else(|| q.strip_prefix(r"%~dp0\")))?;
    if target.to_ascii_lowercase().ends_with(".exe") {
        return Some((None, target.to_string()));
    }
    let interpreter = if quoted.contains(&"%_prog%") {
        // npm: `SET "_prog=%dp0%\node.exe"` when Node sits beside the shim, else `SET "_prog=node"`.
        text.lines().filter_map(|l| l.trim().strip_prefix("SET \"_prog=")?.strip_suffix('"')).find(|p| !p.contains('%'))?
    } else {
        // pnpm: `"%~dp0\node.exe"  "%~dp0\..\pnpm\bin\pnpm.cjs" %*`.
        quoted.iter().any(|q| q.to_ascii_lowercase().ends_with(r"\node.exe")).then_some("node")?
    };
    Some((Some(interpreter.to_string()), target.to_string()))
}

/// The `.exe` a `.cmd` wrapper starts when that is all it does: `"C:\…\codex.exe" %*`, written by
/// hand or by another tool, with only `@echo off`, comments and blank lines besides. Through
/// `cmd.exe` an argument with a line break is refused ("batch file arguments are invalid", #1237),
/// `%` is expanded and the line stops at 8191 characters, so starting the `.exe` is better and the
/// same. Anything more in the file (a `set`, arguments before `%*`, a `%` in the path) and the
/// wrapper is run as it is.
#[cfg(any(windows, test))]
fn exe_wrapper_target(text: &str) -> Option<String> {
    let mut target = None;
    for line in text.lines().map(str::trim) {
        let lower = line.to_ascii_lowercase();
        let lower = lower.strip_prefix('@').unwrap_or(&lower);
        if line.is_empty() || lower == "echo off" || lower == "rem" || lower.starts_with("rem ") || lower.starts_with("::") {
            continue;
        }
        let (path, rest) = line.strip_prefix('@').unwrap_or(line).strip_prefix('"')?.split_once('"')?;
        let drive = path.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && path.get(1..3) == Some(":\\");
        let full = drive || path.starts_with(r"\\");
        if target.is_some() || rest.trim() != "%*" || !full || path.contains('%') || !path.to_ascii_lowercase().ends_with(".exe") {
            return None;
        }
        target = Some(path.to_string());
    }
    target
}

/// The `bin` folders of the Node versions nvm installed, newest first: npm puts the agents it
/// installs there, and only a shell that loaded nvm has them on `PATH` (#112).
#[cfg(unix)]
fn nvm_bins(home: &Path) -> Vec<PathBuf> {
    newest_versions(&home.join(".nvm/versions/node")).into_iter().map(|v| v.join("bin")).collect()
}

/// nvm for Windows' version folders (`node.exe` and the agents npm installed with it), newest
/// first. `PATH` reaches them only through the link `nvm use` makes (`C:\Program Files\nodejs`),
/// which is gone when no version is in use; npm shims then had no Node and failed with "batch file
/// arguments are invalid".
#[cfg(windows)]
fn nvm_bins(home: &Path) -> Vec<PathBuf> {
    let root = std::env::var_os("NVM_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(r"AppData\Roaming\nvm"));
    newest_versions(&root)
}

#[cfg(not(any(unix, windows)))]
fn nvm_bins(_home: &Path) -> Vec<PathBuf> {
    Vec::new()
}

/// The `v<number>…` folders in `root`, newest version first, at most eight.
#[cfg(any(unix, windows))]
fn newest_versions(root: &Path) -> Vec<PathBuf> {
    let Ok(versions) = std::fs::read_dir(root) else { return Vec::new() };
    let mut found: Vec<(Vec<u64>, PathBuf)> = versions
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let number = name.strip_prefix('v').filter(|n| n.starts_with(|c: char| c.is_ascii_digit()))?;
            let path = e.path();
            path.is_dir().then(|| (number.split('.').map(|n| n.parse().unwrap_or(0)).collect(), path))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    found.into_iter().map(|(_, dir)| dir).take(8).collect()
}

/// Where npm puts what it installs globally when the person moved its prefix
/// (`npm config set prefix …` writes `prefix=` to `~/.npmrc`, and `NPM_CONFIG_PREFIX` sets it
/// too): the prefix itself on Windows, its `bin` folder elsewhere (#132).
fn npm_global_bin(home: &Path) -> Option<PathBuf> {
    let set = ["NPM_CONFIG_PREFIX", "npm_config_prefix"].iter().find_map(std::env::var_os).filter(|v| !v.is_empty());
    let prefix = set.map(PathBuf::from).or_else(|| npmrc_prefix(home))?;
    Some(if cfg!(windows) { prefix } else { prefix.join("bin") })
}

/// The `prefix` set in `~/.npmrc`, if any (the last one wins, as in npm).
fn npmrc_prefix(home: &Path) -> Option<PathBuf> {
    let file = home.join(".npmrc");
    let text = std::fs::metadata(&file).ok().filter(|m| m.len() <= 64 * 1024).and_then(|_| std::fs::read_to_string(&file).ok())?;
    let value = text.lines().rev().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == "prefix")?.1.trim().trim_matches('"');
    if value.is_empty() {
        return None;
    }
    Some(match value.strip_prefix("~/").or_else(|| value.strip_prefix("~\\")) {
        Some(rest) => home.join(rest),
        None => PathBuf::from(value),
    })
}

fn with_usual_dirs(path: Option<OsString>, home: Option<PathBuf>, local_data: Option<PathBuf>) -> Option<OsString> {
    let mut dirs: Vec<PathBuf> = path.as_ref().map(|p| std::env::split_paths(p).collect()).unwrap_or_default();
    let before = dirs.len();
    let in_home = home.iter().flat_map(|h| USUAL_HOME_DIRS.iter().map(move |d| h.join(d)).chain(nvm_bins(h)).chain(npm_global_bin(h)));
    // Antigravity's per-user installer may use a redirected LOCALAPPDATA directory, and its CLI
    // (`agy.exe`) sits in `agy\bin` or straight in the `Antigravity` folder, depending on the
    // installer's version (#62, #737). Only a file named `agy` matches, so the desktop app's own
    // `Antigravity.exe` is never taken for the CLI.
    let local_data =
        local_data.filter(|p| !p.as_os_str().is_empty()).or_else(|| home.as_ref().filter(|_| cfg!(windows)).map(|h| h.join("AppData").join("Local")));
    let antigravity = local_data.into_iter().flat_map(|p| [p.join("agy").join("bin"), p.join("Antigravity")]);
    for d in in_home.chain(antigravity).chain(USUAL_DIRS.iter().map(PathBuf::from)) {
        if !dirs.contains(&d) {
            dirs.push(d);
        }
    }
    if dirs.len() == before {
        return path;
    }
    std::env::join_paths(&dirs).ok().or(path)
}

/// Like [`which_all`] with an explicit search path.
pub fn which_in(name: &str, path: Option<OsString>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if name.is_empty() {
        return out;
    }
    let as_path = Path::new(name);
    if as_path.components().count() > 1 || as_path.is_absolute() {
        for c in candidates(as_path) {
            if is_executable(&c) {
                out.push(c);
                break;
            }
        }
        return out;
    }
    let Some(path) = path else { return out };
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        for c in candidates(&dir.join(name)) {
            if is_executable(&c) && !out.contains(&c) {
                out.push(c);
                break;
            }
        }
    }
    out
}

/// File name of the command-line binary (`craft-anything-cli`, plus `.exe` on Windows).
pub fn cli_file_name() -> String {
    format!("craft-anything-cli{}", std::env::consts::EXE_SUFFIX)
}

/// Absolute path of the installed `craft-anything-cli`: this executable when it is the CLI, else
/// a sibling of this executable (the desktop app ships next to it), else the first one on `PATH`.
pub fn self_cli_path() -> Option<PathBuf> {
    // The prefix is dropped from the path found, not from this executable's: a sibling is longer,
    // and only its own length says whether it still fits without the prefix.
    find_cli().map(plain)
}

fn find_cli() -> Option<PathBuf> {
    let name = cli_file_name();
    if let Some(exe) = std::env::current_exe().ok().map(|p| std::fs::canonicalize(&p).unwrap_or(p)) {
        if exe.file_name().is_some_and(|n| n == name.as_str()) {
            return Some(exe);
        }
        if let Some(dir) = exe.parent() {
            let sib = dir.join(&name);
            if is_executable(&sib) {
                return Some(sib);
            }
            // test binaries live in target/<profile>/deps; the CLI is one level up
            if let Some(up) = dir.parent().map(|d| d.join(&name))
                && is_executable(&up)
            {
                return Some(up);
            }
            if let Some(profile) = new_layout_profile_dir(dir).map(|d| d.join(&name))
                && is_executable(&profile)
            {
                return Some(profile);
            }
        }
    }
    which("craft-anything-cli")
}

/// `target/<profile>` for a test binary in Cargo's newer build-dir layout (Rust 1.100), which puts
/// it in `target/<profile>/build/<package>/<hash>/out` while the CLI is still built into
/// `target/<profile>`. Without this, every test that starts the helper found none.
fn new_layout_profile_dir(dir: &Path) -> Option<&Path> {
    let build = dir.ancestors().nth(3)?;
    (dir.file_name().is_some_and(|n| n == "out") && build.file_name().is_some_and(|n| n == "build")).then_some(build.parent()).flatten()
}

/// `path` without the `\\?\` prefix `canonicalize` adds on Windows, so clients that can't start
/// a program from the extended-length form (Hermes, #1167) get a path they can. Elsewhere, and
/// whenever the plain form could name another file (see [`plain_form`]), the path is unchanged.
fn plain(path: PathBuf) -> PathBuf {
    if !cfg!(windows) {
        return path;
    }
    path.to_str().and_then(plain_form).map_or(path, PathBuf::from)
}

/// The plain form of an extended-length path: `\\?\C:\x` is `C:\x` and `\\?\UNC\server\share\x`
/// is `\\server\share\x`. `None` (keep the prefix) for any other text, or when dropping it would
/// change what the path names: it is 260 characters or longer (MAX_PATH), or a component ends
/// with a dot or space, is `.` or `..`, holds a `/`, `?` or `:`, or is a device name (`CON`,
/// `NUL`, `CONOUT$`, `COM0`, `LPT²`…), all of which only the prefixed form keeps literally.
fn plain_form(path: &str) -> Option<String> {
    let (plain, rest) = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        (format!(r"\\{rest}"), rest)
    } else {
        let rest = path.strip_prefix(r"\\?\")?;
        let b = rest.as_bytes();
        let drive = b.first().is_some_and(u8::is_ascii_alphabetic) && b.get(1) == Some(&b':') && b.get(2) == Some(&b'\\');
        if !drive {
            return None;
        }
        (rest.to_string(), rest.get(3..)?)
    };
    let device = |c: &str| {
        let stem = c.split('.').next().unwrap_or(c).trim_end().to_ascii_uppercase();
        let numbered = |p: &str| stem.strip_prefix(p).is_some_and(|n| matches!(n, "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"));
        matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") || numbered("COM") || numbered("LPT")
    };
    let odd = |c: &str| c.is_empty() || c == "." || c == ".." || c.ends_with('.') || c.ends_with(' ') || c.contains(['/', '?', ':']) || device(c);
    if rest.is_empty() || rest.split('\\').any(odd) || plain.encode_utf16().count() >= 260 {
        return None;
    }
    Some(plain)
}

#[cfg(windows)]
fn candidates(base: &Path) -> Vec<PathBuf> {
    let mut v = Vec::new();
    // A file with no extension can't be started by name (CreateProcess only adds `.exe`), and npm
    // puts an extensionless POSIX shell script beside every `.cmd` shim it installs. Try the kinds
    // Windows runs first, then the bare name, so `which` returns what the system would start (#41).
    if base.extension().is_none() {
        let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".into());
        for e in exts.split(';').filter(|e| !e.is_empty()) {
            let mut s = base.as_os_str().to_os_string();
            // Lower case, as installers name their files; PATHEXT spells them `.EXE`.
            s.push(e.to_ascii_lowercase());
            v.push(PathBuf::from(s));
        }
    }
    v.push(base.to_path_buf());
    v
}

#[cfg(not(windows))]
fn candidates(base: &Path) -> Vec<PathBuf> {
    vec![base.to_path_buf()]
}

#[cfg(unix)]
fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(p: &Path) -> bool {
    p.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rust 1.100's Cargo runs a test binary from target/<profile>/build/<package>/<hash>/out, so
    /// the CLI one level up wasn't there any more and every test that starts the helper failed.
    #[test]
    fn the_cli_is_found_from_cargos_newer_test_binary_folder() {
        let out = Path::new("/w/target/debug/build/craft-anything-ai/8138ba20ac4e84fe/out");
        assert_eq!(new_layout_profile_dir(out), Some(Path::new("/w/target/debug")));
        for other in ["/w/target/debug/deps", "/w/target/debug/build/ring/9ac8/out/x", "/w/target/debug/elsewhere/p/h/out", "/opt/app", "/out"] {
            assert_eq!(new_layout_profile_dir(Path::new(other)), None, "{other}");
        }
    }

    #[test]
    fn plain_form_drops_the_extended_length_prefix_only_when_it_names_the_same_file() {
        assert_eq!(plain_form(r"\\?\C:\Apps\craft-anything-cli.exe").as_deref(), Some(r"C:\Apps\craft-anything-cli.exe"));
        assert_eq!(plain_form(r"\\?\UNC\server\share\bin\cli.exe").as_deref(), Some(r"\\server\share\bin\cli.exe"));
        // Not extended-length, or not a drive or UNC path behind the prefix: left alone.
        for p in [
            r"C:\Apps\cli.exe",
            r"\\server\share\cli.exe",
            "/opt/bin/cli",
            r"\\.\pipe\x",
            r"\\?\Volume{1234}\x.exe",
            "",
            r"\\?\",
            r"\\?\C:",
            r"\\?\C:\",
            r"\\?\UNC\",
        ] {
            assert_eq!(plain_form(p), None, "{p}");
        }
        // Components only the prefixed form keeps literally.
        for p in [
            r"\\?\C:\Tools.\cli.exe",
            r"\\?\C:\Tools \cli.exe",
            r"\\?\C:\a\..\cli.exe",
            r"\\?\C:\a\.\cli.exe",
            r"\\?\C:\a\\cli.exe",
            r"\\?\C:\a/b\cli.exe",
            r"\\?\C:\NUL\cli.exe",
            r"\\?\C:\x\con.txt",
            r"\\?\C:\x\COM1",
            r"\\?\C:\x\lpt9.exe",
            r"\\?\UNC\srv\share\aux",
            r"\\?\C:\x\COM¹",
            r"\\?\C:\x\lpt³.txt",
            r"\\?\C:\x\CONOUT$",
            r"\\?\C:\x\conin$.log",
            r"\\?\UNC\?\C:\x.exe",
            r"\\?\C:\a?b\x.exe",
            r"\\?\C:\x\LPT0",
            r"\\?\C:\x\com0.exe",
            r"\\?\C:\x\NUL:s",
            r"\\?\C:\x\cli.exe:stream",
        ] {
            assert_eq!(plain_form(p), None, "{p}");
        }
        assert_eq!(plain_form(r"\\?\C:\x\COM10\console.exe").as_deref(), Some(r"C:\x\COM10\console.exe"));
        // Long paths need the prefix (MAX_PATH is 260 with its terminator).
        let at = |n: usize| format!(r"\\?\C:\{}", "a".repeat(n - 3));
        assert_eq!(plain_form(&at(259)).map(|p| p.len()), Some(259));
        assert_eq!(plain_form(&at(260)), None);
    }

    #[cfg(windows)]
    #[test]
    fn this_executable_comes_back_plain_and_still_names_itself() {
        let exe = std::fs::canonicalize(std::env::current_exe().unwrap()).unwrap();
        assert!(exe.to_string_lossy().starts_with(r"\\?\"), "canonicalize adds the prefix: {exe:?}");
        let p = plain(exe.clone());
        if exe.to_str().and_then(plain_form).is_none() {
            assert_eq!(p, exe, "a checkout only the prefixed form names keeps it");
            return;
        }
        assert!(!p.to_string_lossy().starts_with(r"\\?\"), "{p:?}");
        assert!(p.is_file() && std::fs::canonicalize(&p).unwrap() == exe, "{p:?}");
    }

    /// #1167: what setup registers is the plain path whenever that names the same file.
    #[cfg(windows)]
    #[test]
    fn self_cli_path_is_plain_when_it_can_be() {
        let Some(p) = self_cli_path() else { return };
        let canon = std::fs::canonicalize(&p).unwrap();
        if canon.to_str().and_then(plain_form).is_some() {
            assert!(!p.to_string_lossy().starts_with(r"\\?\"), "{p:?}");
        }
    }

    #[test]
    fn finds_sh_and_misses_nonsense() {
        #[cfg(unix)]
        assert!(which("sh").is_some());
        assert!(which("definitely-not-a-program-xyz").is_none());
        assert!(which("").is_none());
    }

    #[cfg(unix)]
    #[test]
    fn usual_dirs_come_after_path() {
        let home = PathBuf::from("/home/u");
        let path = std::env::join_paths([PathBuf::from("/usr/bin"), home.join(".local/bin")]).unwrap();
        let dirs: Vec<PathBuf> = std::env::split_paths(&with_usual_dirs(Some(path), Some(home.clone()), None).unwrap()).collect();
        assert_eq!(dirs[..3], [PathBuf::from("/usr/bin"), home.join(".local/bin"), home.join(".claude/local")]);
        // (Plus npm's prefix where the environment sets one.)
        let npm = usize::from(npm_global_bin(&home).is_some());
        assert_eq!(dirs.len(), 1 + USUAL_HOME_DIRS.len() + USUAL_DIRS.len() + npm, "nothing twice");
        assert!(dirs.contains(&PathBuf::from("/opt/homebrew/bin")));
        // No PATH and no home folder: the system-wide folders still.
        assert_eq!(with_usual_dirs(None, None, None), std::env::join_paths(USUAL_DIRS).ok());
    }

    /// #112: agents npm installed under nvm, newest Node first, after the usual folders.
    #[cfg(unix)]
    #[test]
    fn nvm_node_versions_are_searched_newest_first() {
        let home = tempfile::tempdir().unwrap();
        for v in ["v9.11.2", "v20.10.0", "v18.19.1"] {
            std::fs::create_dir_all(home.path().join(".nvm/versions/node").join(v).join("bin")).unwrap();
        }
        let bins = nvm_bins(home.path());
        let node = home.path().join(".nvm/versions/node");
        assert_eq!(bins, [node.join("v20.10.0/bin"), node.join("v18.19.1/bin"), node.join("v9.11.2/bin")]);
        let dirs: Vec<PathBuf> = std::env::split_paths(&with_usual_dirs(None, Some(home.path().to_path_buf()), None).unwrap()).collect();
        assert!(dirs.contains(&home.path().join(".opencode/bin")));
        assert!(dirs.contains(&home.path().join(".grok/bin")));
        let at = |p: &Path| dirs.iter().position(|d| d == p).unwrap();
        assert!(at(&home.path().join(".cargo/bin")) < at(&bins[0]) && at(&bins[2]) < at(Path::new("/usr/local/bin")));
        assert!(nvm_bins(Path::new("/nowhere")).is_empty());
    }

    /// Grok Build installs `grok` in `~/.grok/bin`, which a Dock-launched app does not have on PATH.
    #[cfg(unix)]
    #[test]
    fn grok_in_its_home_bin_is_found() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let bin = home.join(".grok/bin");
        std::fs::create_dir_all(&bin).unwrap();
        let program = bin.join("grok");
        std::fs::write(&program, b"#!/bin/sh\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(which_in("grok", with_usual_dirs(None, Some(home), None)).as_slice(), std::slice::from_ref(&program));
    }

    /// #116: Hermes Agent's Windows installer puts `hermes.exe` in `%LOCALAPPDATA%\hermes\bin` and
    /// adds that to the user's PATH, which an app started before the install (or by a launcher)
    /// doesn't have, so Settings found no Hermes.
    #[cfg(windows)]
    #[test]
    fn hermes_agents_windows_folder_is_searched() {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join(r"AppData\Local\hermes\bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("hermes.exe"), "").unwrap();
        let path = with_usual_dirs(None, Some(home.path().to_path_buf()), None);
        assert_eq!(which_in("hermes", path), [bin.join("hermes.exe")]);
    }

    /// #132: npm's own prefix setting says where it put the agents it installed.
    #[test]
    fn npms_prefix_setting_is_searched() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(npmrc_prefix(home.path()), None, "no .npmrc");
        let rc = home.path().join(".npmrc");
        std::fs::write(&rc, "; a comment\nregistry=https://registry.npmjs.org/\nprefix=/old\nprefix = \"C:\\npm-global\"\n").unwrap();
        assert_eq!(npmrc_prefix(home.path()), Some(PathBuf::from("C:\\npm-global")), "the last one wins, quotes dropped");
        std::fs::write(&rc, "prefix=~/.npm-packages\n").unwrap();
        assert_eq!(npmrc_prefix(home.path()), Some(home.path().join(".npm-packages")));
        std::fs::write(&rc, "#prefix=/commented\nprefix=\n").unwrap();
        assert_eq!(npmrc_prefix(home.path()), None);
        // Unless npm's prefix is set in the environment, the folder it names is searched.
        if std::env::var_os("NPM_CONFIG_PREFIX").is_none() && std::env::var_os("npm_config_prefix").is_none() {
            std::fs::write(&rc, "prefix=~/.npm-packages\n").unwrap();
            let bin = if cfg!(windows) { home.path().join(".npm-packages") } else { home.path().join(".npm-packages").join("bin") };
            let dirs: Vec<PathBuf> = std::env::split_paths(&with_usual_dirs(None, Some(home.path().to_path_buf()), None).unwrap()).collect();
            assert!(dirs.contains(&bin), "{dirs:?}");
        }
    }

    #[test]
    fn antigravity_is_found_in_redirected_local_app_data() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("profile");
        let local_data = dir.path().join("redirected-local-data");
        let bin = local_data.join("agy").join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let program = bin.join(if cfg!(windows) { "agy.exe" } else { "agy" });
        std::fs::write(&program, b"fixture").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let path = with_usual_dirs(None, Some(home.clone()), Some(local_data.clone()));
        assert_eq!(which_in("agy", path.clone()).as_slice(), std::slice::from_ref(&program));
        let dirs: Vec<_> = std::env::split_paths(path.as_ref().unwrap()).collect();
        assert!(!dirs.contains(&home.join("AppData").join("Local").join("agy").join("bin")));
        // A folder already in PATH retains its position and is not added again.
        let explicit = std::env::join_paths([&bin]).unwrap();
        let path = with_usual_dirs(Some(explicit), Some(home), Some(local_data)).unwrap();
        let dirs: Vec<_> = std::env::split_paths(&path).collect();
        assert_eq!(dirs.first(), Some(&bin));
        assert_eq!(dirs.iter().filter(|d| *d == &bin).count(), 1);
        assert_eq!(which_in("agy", Some(path)), [program]);
    }

    /// #737: the Antigravity installer may put `agy.exe` straight in the `Antigravity` folder, and
    /// that folder's desktop app (`Antigravity.exe`) must not be taken for the CLI.
    #[test]
    fn antigravity_cli_next_to_its_folder_is_found_but_not_the_desktop_app() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("profile");
        let local_data = dir.path().join("local");
        let folder = local_data.join("Antigravity");
        std::fs::create_dir_all(&folder).unwrap();
        let app = folder.join("Antigravity.exe");
        std::fs::write(&app, b"desktop").unwrap();
        let path = with_usual_dirs(None, Some(home.clone()), Some(local_data.clone()));
        assert!(which_in("agy", path.clone()).is_empty(), "no CLI yet");
        let program = folder.join(if cfg!(windows) { "agy.exe" } else { "agy" });
        std::fs::write(&program, b"cli").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        assert_eq!(which_in("agy", path.clone()).as_slice(), std::slice::from_ref(&program));
    }

    #[cfg(windows)]
    #[test]
    fn usual_windows_dirs_come_after_path() {
        let home = PathBuf::from(r"C:\Users\u");
        let path = OsString::from(r"C:\Windows;C:\Users\u\.local\bin");
        let dirs: Vec<PathBuf> = std::env::split_paths(&with_usual_dirs(Some(path), Some(home.clone()), None).unwrap()).collect();
        assert_eq!(dirs[..3], [PathBuf::from(r"C:\Windows"), home.join(r".local\bin"), home.join(r"AppData\Roaming\npm")]);
        // PATH's two less `.local\bin`, which is a usual one too; Antigravity's two folders (plus
        // npm's prefix where the environment sets one, and nvm for Windows' versions where
        // `NVM_HOME` names them).
        let npm = usize::from(npm_global_bin(&home).is_some());
        let nvm = nvm_bins(&home).len();
        assert_eq!(dirs.len(), 1 + USUAL_HOME_DIRS.len() + 2 + USUAL_DIRS.len() + npm + nvm, "nothing twice");
        assert!(dirs.contains(&home.join("AppData").join("Local").join("agy").join("bin")));
        assert!(dirs.contains(&home.join("AppData").join("Local").join("Antigravity")));
        // Each way Claude Code installs on Windows: native, npm, WinGet (per user and machine-wide).
        assert!(dirs.contains(&home.join(r"AppData\Local\Microsoft\WinGet\Links")));
        assert!(dirs.contains(&home.join(r".grok\bin")));
        assert!(dirs.contains(&PathBuf::from(r"C:\Program Files\WinGet\Links")));
    }

    const NPM_SHIM: &str = "@ECHO off\r\nGOTO start\r\n:find_dp0\r\nSET dp0=%~dp0\r\nEXIT /b\r\n:start\r\nSETLOCAL\r\nCALL :find_dp0\r\n\r\n\
        IF EXIST \"%dp0%\\node.exe\" (\r\n  SET \"_prog=%dp0%\\node.exe\"\r\n) ELSE (\r\n  SET \"_prog=node\"\r\n  SET PATHEXT=%PATHEXT:;.JS;=;%\r\n)\r\n\r\n\
        endLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@google\\gemini-cli\\dist\\index.js\" %*\r\n";

    #[test]
    fn npm_shims_name_their_script() {
        assert_eq!(parse_npm_shim(NPM_SHIM), Some((Some("node".into()), r"node_modules\@google\gemini-cli\dist\index.js".into())));
        let pnpm = "@SETLOCAL\n@IF EXIST \"%~dp0\\node.exe\" (\n  \"%~dp0\\node.exe\"  \"%~dp0\\..\\pnpm\\bin\\pnpm.cjs\" %*\n) ELSE (\n  node  \"%~dp0\\..\\pnpm\\bin\\pnpm.cjs\" %*\n)\n";
        assert_eq!(parse_npm_shim(pnpm), Some((Some("node".into()), r"..\pnpm\bin\pnpm.cjs".into())));
        let exe = "@ECHO off\nendLocal & \"%dp0%\\node_modules\\tool\\bin\\tool.exe\"   %*\n";
        assert_eq!(parse_npm_shim(exe), Some((None, r"node_modules\tool\bin\tool.exe".into())));
        // Not a shim we know: the batch file is run as it is.
        assert_eq!(parse_npm_shim("@echo off\r\n\"%NODE_EXE%\" \"%NPM_CLI_JS%\" %*\r\n"), None);
        assert_eq!(parse_npm_shim("@echo off\r\necho hi\r\n"), None);
    }

    /// #1237: a hand-written `codex.cmd` that runs Codex's own `codex.exe` by its full path.
    const EXE_WRAPPER: &str = "@echo off\r\nrem Points \"codex\" at the real binary.\r\n:: and a second comment\r\n\r\n\"C:\\npm\\node_modules\\@openai\\codex\\bin\\codex.exe\" %*\r\n";

    #[test]
    fn a_cmd_that_only_runs_an_exe_names_it() {
        let exe = r"C:\npm\node_modules\@openai\codex\bin\codex.exe";
        assert_eq!(exe_wrapper_target(EXE_WRAPPER).as_deref(), Some(exe));
        assert_eq!(parse_npm_shim(EXE_WRAPPER), Some((None, exe.into())));
        assert_eq!(exe_wrapper_target("@\"\\\\server\\share\\tool.EXE\"   %*\n").as_deref(), Some(r"\\server\share\tool.EXE"), "a UNC path, and @ before it");
        // Anything more and the wrapper is run as it is, so nothing it does is lost.
        for (text, why) in [
            ("@echo off\nset CODEX_HOME=C:\\x\n\"C:\\x\\codex.exe\" %*\n", "it sets something first"),
            ("@echo off\n\"C:\\x\\codex.exe\" --profile work %*\n", "arguments before %*"),
            ("@echo off\n\"C:\\x\\codex.exe\" %* --extra\n", "arguments after %*"),
            ("@echo off\n\"C:\\x\\codex.exe\"\n", "it drops the arguments"),
            ("@echo off\n\"%LOCALAPPDATA%\\x\\codex.exe\" %*\n", "a % in the path"),
            ("@echo off\n\"x\\codex.exe\" %*\n", "a relative path"),
            ("@echo off\n\"C:\\x\\codex.cmd\" %*\n", "not an .exe"),
            ("@echo off\n\"C:\\x\\a.exe\" %*\n\"C:\\x\\b.exe\" %*\n", "two programs"),
            ("@echo off\nrem nothing here\n", "nothing runs"),
        ] {
            assert_eq!(exe_wrapper_target(text), None, "{why}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn a_cmd_that_only_runs_an_exe_starts_the_exe() {
        let dir = tempfile::tempdir().unwrap();
        let bin = tempfile::tempdir().unwrap();
        let exe = bin.path().join("codex.exe");
        let wrapper = |exe: &Path| format!("@echo off\r\nrem a wrapper\r\n\"{}\" %*\r\n", exe.display());
        std::fs::write(dir.path().join("codex.cmd"), wrapper(&exe)).unwrap();
        // The .exe isn't there: the wrapper itself, as before.
        assert_eq!(launch_target(dir.path().join("codex").as_os_str()), (dir.path().join("codex.cmd").into_os_string(), Vec::new()));
        std::fs::write(&exe, "").unwrap();
        assert_eq!(launch_target(dir.path().join("codex").as_os_str()), (exe.into_os_string(), Vec::new()));
    }

    #[cfg(windows)]
    #[test]
    fn npm_shims_start_their_script_with_node() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("gemini.cmd");
        std::fs::write(&shim, NPM_SHIM).unwrap();
        // The shim's script missing: the batch file itself.
        assert_eq!(launch_target(shim.as_os_str()), (shim.clone().into_os_string(), Vec::new()));
        let script = dir.path().join(r"node_modules\@google\gemini-cli\dist\index.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "").unwrap();
        std::fs::write(dir.path().join("node.exe"), "").unwrap();
        // Found without its extension too, as a bare name is.
        assert_eq!(launch_target(dir.path().join("gemini").as_os_str()), (dir.path().join("node.exe").into_os_string(), vec![script.into_os_string()]));
        // An .exe is started by the path it was found at.
        assert_eq!(launch_target(dir.path().join("node").as_os_str()), (dir.path().join("node.exe").into_os_string(), Vec::new()));
    }

    /// nvm for Windows keeps each version's `node.exe` in `v<version>` beside its own files.
    #[cfg(any(unix, windows))]
    #[test]
    fn node_versions_are_listed_newest_first_without_other_files() {
        let root = tempfile::tempdir().unwrap();
        for v in ["v18.17.0", "v22.22.1", "v20.18.0", "v20.20.1", "temp", "vendor"] {
            std::fs::create_dir_all(root.path().join(v)).unwrap();
        }
        for f in ["elevate.cmd", "nvm.exe", "settings.txt", "v9.0.0"] {
            std::fs::write(root.path().join(f), "").unwrap();
        }
        let names: Vec<String> = newest_versions(root.path()).iter().filter_map(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["v22.22.1", "v20.20.1", "v20.18.0", "v18.17.0"]);
        assert!(newest_versions(&root.path().join("missing")).is_empty());
    }

    /// An npm shim with its script but no Node anywhere: the interpreter is named, so the error can
    /// say what's missing instead of "batch file arguments are invalid".
    #[cfg(windows)]
    #[test]
    fn a_shim_without_node_names_it() {
        let dir = tempfile::tempdir().unwrap();
        let shim = dir.path().join("claude.cmd");
        std::fs::write(&shim, NPM_SHIM).unwrap();
        let empty = tempfile::tempdir().unwrap();
        let path = || Some(empty.path().as_os_str().to_owned());
        assert_eq!(missing_interpreter_in(&shim, path()), None, "no script: not a shim we can run anyway");
        let script = dir.path().join(r"node_modules\@google\gemini-cli\dist\index.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "").unwrap();
        assert_eq!(missing_interpreter_in(&shim, path()), Some("node".into()));
        std::fs::write(empty.path().join("node.exe"), "").unwrap();
        assert_eq!(missing_interpreter_in(&shim, path()), None, "Node on the search path");
        std::fs::remove_file(empty.path().join("node.exe")).unwrap();
        std::fs::write(dir.path().join("node.exe"), "").unwrap();
        assert_eq!(missing_interpreter_in(&shim, path()), None, "Node beside the shim");
        assert_eq!(missing_interpreter_in(&dir.path().join("node.exe"), path()), None, "not a shim");
    }

    /// A prompt with line breaks and quotes reaches the agent whole (needs Node; skipped without).
    #[cfg(windows)]
    #[test]
    fn a_shim_gets_a_multiline_prompt() {
        if which("node").is_none() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join(r"node_modules\@google\gemini-cli\dist\index.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "process.stdout.write(JSON.stringify(process.argv.slice(2)))").unwrap();
        std::fs::write(dir.path().join("gemini.cmd"), NPM_SHIM).unwrap();
        let prompt = "Make a game.\nIt says \"hi\" & 100% works | <ok>";
        let out = crate::Process::new(dir.path().join("gemini")).args(["-p", prompt]).run_checked().unwrap();
        assert_eq!(serde_json::from_str::<Vec<String>>(&out.stdout_text()).unwrap(), ["-p", prompt]);
    }

    /// #41: npm writes an extensionless POSIX script beside its `.cmd` shim, and Windows can't start
    /// that script by name, so a bare name must find the shim first.
    #[cfg(windows)]
    #[test]
    fn a_bare_name_finds_what_windows_can_start() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tool"), "#!/bin/sh\n").unwrap();
        std::fs::write(dir.path().join("tool.cmd"), "@echo off\n").unwrap();
        let path = || Some(dir.path().as_os_str().to_owned());
        // The extension comes from PATHEXT, so its case need not match the file's.
        let found = which_in("tool", path());
        assert_eq!(found.len(), 1);
        assert!(found[0].file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case("tool.cmd")), "the shim, not the POSIX script: {found:?}");
        // With only the script there, it's still reported (and starting it fails with the program's name).
        std::fs::remove_file(dir.path().join("tool.cmd")).unwrap();
        assert_eq!(which_in("tool", path()), vec![dir.path().join("tool")]);
    }

    #[cfg(unix)]
    #[test]
    fn explicit_search_path() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("tool");
        std::fs::write(&p, "#!/bin/sh\n").unwrap();
        assert!(which_in("tool", Some(dir.path().as_os_str().to_owned())).is_empty());
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(which_in("tool", Some(dir.path().as_os_str().to_owned())), vec![p]);
    }
}
