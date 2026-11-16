//! `cargo xtask <layers | locks | status | i18n-coverage | tools [--check] | wasm | licenses | schemas [--check] | pending [--count] | secrets [--history] | ci | version [set X.Y.Z] | ico <out.ico> <png>...>`
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod i18n_coverage;
mod ico;
mod locks;
mod secrets;
mod source_policy;
mod status;

/// Workspace crates and their layer. A crate may depend only on strictly lower layers
/// (L0 format crates depend on nothing in the workspace).
const LAYERS: &[(&str, u8)] = &[
    ("craft-anything-evidence", 0),
    ("craft-anything-formats", 0),
    ("craft-anything-disasm", 1),
    ("craft-anything-js", 1),
    ("craft-anything-analysis", 2),
    ("craft-anything-platform", 2),
    ("craft-anything-engines", 3),
    ("craft-anything-engine", 4),
    ("craft-anything-ai", 5),
    ("craft-anything-mcp", 5),
    ("craft-anything-setup", 5),
    ("craft-anything-ui-egui", 6),
];

/// Crates that must build for wasm32-unknown-unknown (static analysis in the browser).
const WASM_CRATES: &[&str] = &["craft-anything-evidence", "craft-anything-formats", "craft-anything-disasm", "craft-anything-js", "craft-anything-analysis"];

/// UI toolkits only the UI layer may use.
const UI_ONLY: &[&str] = &["egui", "eframe", "winit", "rfd", "egui_extras", "egui_kittest"];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = workspace_root();
    let ok = match args.first().map(String::as_str) {
        Some("layers") => layers(&root),
        Some("locks") => locks::check(&root),
        Some("status") => status::check(&root),
        Some("i18n-coverage") => i18n_coverage::check(&root),
        Some("tools") => tools(&root, args.iter().any(|a| a == "--check")),
        Some("schemas") => schemas(&root, args.iter().any(|a| a == "--check")),
        Some("wasm") => wasm(),
        Some("licenses") => licenses(),
        Some("version") => version(&root, args.get(1).map(String::as_str), args.get(2).map(String::as_str)),
        Some("pending") => pending(&root, args.iter().any(|a| a == "--count")),
        Some("secrets") => secrets::secrets(&root, args.iter().any(|a| a == "--history")),
        Some("ico") => {
            let rest: Vec<&str> = args.iter().skip(1).map(String::as_str).collect();
            ico::run(&rest).map_err(|e| eprintln!("ico: {e}")).is_ok()
        }
        Some("ci") => {
            status::check(&root)
                && layers(&root)
                && locks::check(&root)
                && i18n_coverage::check(&root)
                && secrets::secrets(&root, false)
                && run("cargo", &["fmt", "--all", "--check"])
                && run("cargo", &["clippy", "--workspace", "--all-targets", "--", "-D", "warnings"])
                && run("cargo", &["test", "--workspace"])
                // The off-by-default WASM plugin host, built and tested on its own.
                && run("cargo", &["clippy", "-p", "craft-anything-engine", "--features", "wasm-plugins", "--all-targets", "--", "-D", "warnings"])
                && run("cargo", &["test", "-p", "craft-anything-engine", "--features", "wasm-plugins"])
                && tools(&root, true)
                && schemas(&root, true)
                && wasm()
        }
        _ => {
            eprintln!(
                "usage: cargo xtask <layers | locks | status | i18n-coverage | tools [--check] | wasm | licenses | schemas [--check] | pending [--count] | secrets [--history] | ci | version [set X.Y.Z] | ico <out.ico> <png>...>"
            );
            false
        }
    };
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

/// `version`: print `[workspace.package] version` (the single source of the app version).
/// `version set X.Y.Z[-pre]`: rewrite it and refresh the workspace entries in Cargo.lock.
fn version(root: &Path, sub: Option<&str>, value: Option<&str>) -> bool {
    let path = root.join("Cargo.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        eprintln!("can't read {}", path.display());
        return false;
    };
    // The line index of `version = "..."` inside [workspace.package], and its value.
    let mut in_pkg = false;
    let mut found: Option<(usize, String)> = None;
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.starts_with('[') {
            in_pkg = t == "[workspace.package]";
            continue;
        }
        if in_pkg && let Some(v) = t.strip_prefix("version = \"").and_then(|r| r.strip_suffix('"')) {
            found = Some((i, v.to_string()));
            break;
        }
    }
    let Some((line_no, cur)) = found else {
        eprintln!("no version in [workspace.package] of {}", path.display());
        return false;
    };
    match (sub, value) {
        (None, _) => {
            println!("{cur}");
            true
        }
        (Some("set"), Some(v)) => {
            let core = v.split(['-', '+']).next().unwrap_or_default();
            let parts: Vec<&str> = core.split('.').collect();
            let valid = parts.len() == 3 && parts.iter().all(|p| !p.is_empty() && p.parse::<u64>().is_ok()) && !v.chars().any(char::is_whitespace);
            if !valid {
                eprintln!("version must look like 1.2.3 or 1.2.3-rc.1, not {v:?}");
                return false;
            }
            let mut out: Vec<String> = text.lines().map(str::to_string).collect();
            if let Some(line) = out.get_mut(line_no) {
                *line = format!("version = \"{v}\"");
            }
            let mut new = out.join("\n");
            if text.ends_with('\n') {
                new.push('\n');
            }
            if let Err(e) = std::fs::write(&path, new) {
                eprintln!("can't write {}: {e}", path.display());
                return false;
            }
            eprintln!("version {cur} -> {v}");
            run("cargo", &["update", "--workspace", "--offline"]) || run("cargo", &["update", "--workspace"])
        }
        _ => {
            eprintln!("usage: cargo xtask version [set X.Y.Z]");
            false
        }
    }
}

fn workspace_root() -> PathBuf {
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    here.parent().map(Path::to_path_buf).unwrap_or(here)
}

fn run(cmd: &str, args: &[&str]) -> bool {
    eprintln!("$ {cmd} {}", args.join(" "));
    Command::new(cmd).args(args).status().map(|s| s.success()).unwrap_or(false)
}

fn manifest_deps(path: &Path) -> (String, Vec<String>) {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let v: toml::Table = toml::from_str(&text).unwrap_or_default();
    let name = v.get("package").and_then(|p| p.get("name")).and_then(|n| n.as_str()).unwrap_or_default().to_string();
    let mut deps = Vec::new();
    let mut collect = |t: Option<&toml::Value>| {
        if let Some(t) = t.and_then(|t| t.as_table()) {
            deps.extend(t.keys().cloned());
        }
    };
    collect(v.get("dependencies"));
    if let Some(targets) = v.get("target").and_then(|t| t.as_table()) {
        for t in targets.values() {
            collect(t.get("dependencies"));
        }
    }
    (name, deps)
}

/// Apps (`apps/*`) sit above every crate: they may depend on any crate, but only an app built on
/// the UI crate may use a UI toolkit, and test helpers stay out of shipped binaries.
const APP_LAYER: u8 = 7;

/// Apps that are user interfaces of their own. The browser viewer can't build on the UI crate
/// (it pulls in native-only crates), so it draws with egui directly and may use only the
/// wasm-safe crates.
const UI_APPS: &[&str] = &["craft-anything", "craft-anything-web"];
const WASM_APPS: &[&str] = &["craft-anything-web"];

fn layers(root: &Path) -> bool {
    let layer: BTreeMap<&str, u8> = LAYERS.iter().copied().collect();
    let mut ok = true;
    let mut manifests = Vec::new();
    for dir in ["crates", "apps"] {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            eprintln!("layers: cannot read {dir}/");
            return false;
        };
        manifests.extend(entries.flatten().map(|e| (dir == "apps", e.path().join("Cargo.toml"))));
    }
    for (is_app, manifest) in manifests {
        let (name, deps) = manifest_deps(&manifest);
        if name == "craft-anything-testkit" {
            continue;
        }
        let mine = if is_app {
            APP_LAYER
        } else if let Some(&l) = layer.get(name.as_str()) {
            l
        } else {
            eprintln!("layers: {name} is not registered in xtask/src/main.rs LAYERS");
            ok = false;
            continue;
        };
        let ui_allowed = mine == 6 || (is_app && (UI_APPS.contains(&name.as_str()) || deps.iter().any(|d| d == "craft-anything-ui-egui")));
        for d in &deps {
            if let Some(&theirs) = layer.get(d.as_str())
                && theirs >= mine
            {
                eprintln!("layers: {name} (L{mine}) depends on {d} (L{theirs})");
                ok = false;
            }
            if !ui_allowed && UI_ONLY.contains(&d.as_str()) {
                eprintln!("layers: {name} (L{mine}) uses UI toolkit {d}; only ui crates (and apps built on them) may");
                ok = false;
            }
            if WASM_APPS.contains(&name.as_str()) && layer.contains_key(d.as_str()) && !WASM_CRATES.contains(&d.as_str()) {
                eprintln!("layers: browser app {name} depends on {d}, which doesn't build for wasm");
                ok = false;
            }
            if is_app && d == "craft-anything-testkit" {
                eprintln!("layers: app {name} depends on craft-anything-testkit; use it as a dev-dependency only");
                ok = false;
            }
        }
    }
    ok &= process_calls_only_in_platform(root);
    ok &= ui_has_no_side_effects(root);
    ok &= no_real_machine_sessions(root);
    if ok {
        eprintln!("layers: ok");
    }
    ok
}

/// Golden rule 10: only platform and test code may name std::process::Command.
/// Inspect Rust paths and explicit imports, including production items after a test module.
fn process_calls_only_in_platform(root: &Path) -> bool {
    check_source_policy(
        root,
        &["crates", "apps"],
        &["target", "tests", "platform", "xtask"],
        source_policy::Policy::Process,
        "runs a program with std::process::Command; use craft_anything_platform::process (golden rule 10)",
    )
}

/// R1.6: UI src paths may read files, but writes and launches belong behind tools.
/// Explicit module files reachable from this source root are included too.
fn ui_has_no_side_effects(root: &Path) -> bool {
    check_source_policy(
        root,
        &["crates/ui-egui/src"],
        &["target", "tests"],
        source_policy::Policy::Ui,
        "side effect in the UI; do it through a tool so the CLI and MCP get it too (R1.6)",
    )
}

fn check_source_policy(root: &Path, dirs: &[&str], skip: &[&str], policy: source_policy::Policy, reason: &str) -> bool {
    let mut ok = true;
    for dir in dirs {
        let files = match source_policy::rust_files(&root.join(dir), skip) {
            Ok(files) => files,
            Err(e) => {
                eprintln!("layers: {e}");
                ok = false;
                continue;
            }
        };
        let files = match source_policy::read_all(files, root) {
            Ok(files) => files,
            Err(e) => {
                eprintln!("layers: {e}");
                ok = false;
                continue;
            }
        };
        let tests = match source_policy::test_files(&files) {
            Ok(tests) => tests,
            Err(e) => {
                eprintln!("layers: {e}");
                ok = false;
                continue;
            }
        };
        for (file, syntax) in files {
            if !matches!(policy, source_policy::Policy::Session) && file.canonicalize().is_ok_and(|p| tests.contains(&p)) {
                continue;
            }
            for (line, path) in source_policy::problems(&syntax, policy) {
                eprintln!("layers: {}:{line}: `{path}`: {reason}", file.strip_prefix(root).unwrap_or(&file).display());
                ok = false;
            }
        }
    }
    ok
}

fn tools(root: &Path, check: bool) -> bool {
    let registry = craft_anything_engine::Registry::with_all_tools();
    let report = craft_anything_engine::tool_list::report(&registry);
    let n = registry.all().len();
    let path = root.join("docs").join("tools.md");
    if check {
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current != report {
            eprintln!("tools: docs/tools.md is stale; run `cargo xtask tools`");
            return false;
        }
        eprintln!("tools: {n} (up to date)");
        return true;
    }
    let _ = std::fs::create_dir_all(root.join("docs"));
    match std::fs::write(&path, report) {
        Ok(()) => {
            eprintln!("tools: {n} → docs/tools.md");
            true
        }
        Err(e) => {
            eprintln!("tools: {e}");
            false
        }
    }
}

/// `cargo xtask schemas [--check]`: regenerates the JSON Schemas of the files we persist into
/// `docs/schemas/`, or with `--check` fails when they're stale (see docs/file-formats.md).
fn schemas(root: &Path, check: bool) -> bool {
    let dir = root.join("docs").join("schemas");
    let files = [
        ("craft.schema.json", craft_anything_engine::workspace::manifest_schema()),
        ("build-progress.schema.json", craft_anything_engine::workspace::progress::build_progress_schema()),
        ("mechanics.schema.json", craft_anything_engine::mechanics::mechanics_schema()),
        ("self-test.schema.json", craft_anything_engine::self_test::self_test_schema()),
    ];
    let mut ok = true;
    for (name, schema) in files {
        let path = dir.join(name);
        let current = std::fs::read_to_string(&path).unwrap_or_default().replace("\r\n", "\n");
        if current == schema {
            eprintln!("schemas: docs/schemas/{name} (up to date)");
            continue;
        }
        if check {
            eprintln!("schemas: docs/schemas/{name} is stale; run `cargo xtask schemas`");
            ok = false;
            continue;
        }
        match std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &schema)) {
            Ok(()) => eprintln!("schemas: wrote docs/schemas/{name}"),
            Err(e) => {
                eprintln!("schemas: {name}: {e}");
                ok = false;
            }
        }
    }
    ok
}

/// Checks every dependency's licence against `deny.toml` (policy: docs/licences.md).
fn licenses() -> bool {
    let installed = Command::new("cargo").args(["deny", "--version"]).output().map(|o| o.status.success()).unwrap_or(false);
    if !installed {
        eprintln!("licenses: cargo-deny is not installed; install it with `cargo install cargo-deny --locked`");
        return false;
    }
    run("cargo", &["deny", "check", "licenses"])
}

fn wasm() -> bool {
    let mut args = vec!["check", "--target", "wasm32-unknown-unknown"];
    for c in WASM_CRATES {
        args.push("-p");
        args.push(c);
    }
    run("cargo", &args)
}

/// Tests never touch the real machine (docs/pitfalls.md): `Session::default()` points at this
/// computer's config folder and engine store, so code uses `testing::session()` (a temp folder)
/// or `Session::new(settings)` with settings it chose. Comments are fine.
fn no_real_machine_sessions(root: &Path) -> bool {
    check_source_policy(
        root,
        &["crates", "apps"],
        &["target"],
        source_policy::Policy::Session,
        "Session::default() uses this computer's folders; use testing::session() in tests",
    )
}

/// `pending`: every unticked `- [ ]` line in docs/ROADMAP.md, under its `### R…` item (with any
/// indented continuation lines), and the totals. `--count` prints only the totals.
fn pending(root: &Path, count_only: bool) -> bool {
    let Ok(text) = std::fs::read_to_string(root.join("docs/ROADMAP.md")) else {
        eprintln!("pending: can't read docs/ROADMAP.md");
        return false;
    };
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    let mut heading = String::from("(before any item)");
    let mut open: Option<String> = None;
    let flush = |open: &mut Option<String>, groups: &mut Vec<(String, Vec<String>)>, heading: &str| {
        if let Some(line) = open.take() {
            match groups.last_mut() {
                Some((h, items)) if h == heading => items.push(line),
                _ => groups.push((heading.to_string(), vec![line])),
            }
        }
    };
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("### ").or_else(|| line.strip_prefix("## ")) {
            flush(&mut open, &mut groups, &heading);
            heading = h.trim().to_string();
        } else if let Some(item) = line.strip_prefix("- [ ] ") {
            flush(&mut open, &mut groups, &heading);
            open = Some(item.trim().to_string());
        } else if open.is_some() && line.starts_with("      ") {
            if let Some(o) = open.as_mut() {
                o.push(' ');
                o.push_str(line.trim());
            }
        } else {
            flush(&mut open, &mut groups, &heading);
        }
    }
    flush(&mut open, &mut groups, &heading);
    let total: usize = groups.iter().map(|(_, i)| i.len()).sum();
    if !count_only {
        for (h, items) in &groups {
            println!("{h}");
            for i in items {
                println!("  - {i}");
            }
        }
        println!();
    }
    println!("{total} open item(s) under {} roadmap entr{}", groups.len(), if groups.len() == 1 { "y" } else { "ies" });
    true
}

#[cfg(test)]
mod tests {
    use super::workspace_root;

    #[test]
    fn readme_status_does_not_list_first_public_release_as_next() {
        let readme = std::fs::read_to_string(workspace_root().join("README.md")).unwrap();
        let status = readme.split("\n## Status").nth(1).unwrap();
        let status = status.split("\n**Responsible use.**").next().unwrap().replace("\n> ", " ");
        assert!(!status.contains("**Next:** the first public release"), "{status}");
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    fn fixture(path: &str, source: impl AsRef<[u8]>) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("crates")).unwrap();
        std::fs::create_dir_all(dir.path().join("apps")).unwrap();
        if path.starts_with("crates/") || path.starts_with("apps/") {
            let package: PathBuf = Path::new(path).components().take(2).collect();
            std::fs::create_dir_all(dir.path().join(&package)).unwrap();
            std::fs::write(dir.path().join(package).join("Cargo.toml"), "[package]\nname = \"sample\"\nversion = \"0.0.0\"\nedition = \"2024\"\n").unwrap();
        }
        let file = dir.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, source).unwrap();
        dir
    }

    #[test]
    fn process_policy_checks_production_after_test_items() {
        for cfg in ["test", "all(unix, test)", "any(all(test, unix), all(test, windows))"] {
            let source = format!(
                "#[cfg({cfg})] mod tests {{ fn test_only() {{ std::process::Command::new(\"test\"); }} }}\nfn production() {{ std::process::Command::new(\"app\"); }}"
            );
            let d = fixture("crates/sample/src/lib.rs", source);
            assert!(!process_calls_only_in_platform(d.path()), "production after cfg({cfg}) must be checked");
        }
    }

    #[test]
    fn process_policy_leaves_test_code_and_examples_alone() {
        let d = fixture(
            "crates/sample/src/lib.rs",
            r#"
            /* std::process::Command::new("example"); */
            const EXAMPLE: &str = "std::process::Command::new";
            use std::process::{Stdio, ExitCode};
            #[cfg(all(unix, test))]
            mod tests { fn legal() { std::process::Command::new("test"); } }
            fn production() { let _ = ExitCode::SUCCESS; }
        "#,
        );
        assert!(process_calls_only_in_platform(d.path()));
        for path in ["crates/platform/src/lib.rs", "crates/sample/tests/process.rs", "xtask/src/main.rs"] {
            let d = fixture(path, "fn f() { std::process::Command::new(\"test\"); }");
            assert!(process_calls_only_in_platform(d.path()), "{path} is exempt");
        }
    }

    #[test]
    fn a_local_type_named_command_is_not_a_std_process() {
        let d = fixture(
            "crates/sample/src/lib.rs",
            r#"
            mod process { pub struct Command; impl Command { pub fn new() -> Self { Self } } }
            fn production() { let _ = process::Command::new(); }
        "#,
        );
        assert!(process_calls_only_in_platform(d.path()));
    }

    #[test]
    fn process_policy_checks_aliases_and_non_test_cfgs() {
        for source in [
            "use std::process::Command as Child; fn f() { Child::new(\"app\"); }",
            "use std::{process as child}; fn f() { child::Command::new(\"app\"); }",
            "use std as system; fn f() { system::process::Command::new(\"app\"); }",
            "fn f() { ::std :: process :: Command :: new(\"app\"); }",
            "#[cfg(not(test))] fn f() { std::process::Command::new(\"app\"); }",
            "#[cfg(any(test, feature = \"app\"))] fn f() { std::process::Command::new(\"app\"); }",
        ] {
            let d = fixture("apps/sample/src/main.rs", source);
            assert!(!process_calls_only_in_platform(d.path()), "{source}");
        }
    }

    #[test]
    fn ui_policy_checks_items_after_tests_and_nested_sources() {
        for path in ["crates/ui-egui/src/main.rs", "crates/ui-egui/src/nested/write.rs"] {
            let d = fixture(path, "#[cfg(test)] mod tests {}\nfn production() { std::fs::write(\"out\", b\"x\"); }");
            assert!(!ui_has_no_side_effects(d.path()), "{path}");
        }
    }

    #[test]
    fn ui_policy_leaves_test_writes_readers_and_examples_alone() {
        let d = fixture(
            "crates/ui-egui/src/main.rs",
            r#"
            /* std::fs::write("example", b"x"); */
            const EXAMPLE: &str = "File::create fs::write Process::new";
            struct Process;
            fn display_state(_: Option<Process>) {}
            #[cfg(all(unix, test))]
            mod tests { fn legal() { std::fs::write("out", b"x"); } }
            fn production() { let _ = std::fs::read("in"); }
        "#,
        );
        assert!(ui_has_no_side_effects(d.path()));
    }

    #[test]
    fn ui_policy_checks_imported_writes() {
        for source in [
            "use std::fs::write as save; fn f() { save(\"out\", b\"x\"); }",
            "use std::fs as files; fn f() { files::write(\"out\", b\"x\"); }",
            "use std::fs::{File as Output}; fn f() { Output::create(\"out\"); }",
            "use craft_anything_platform::process::Process as Child; fn f() { Child::new(\"app\"); }",
        ] {
            let d = fixture("crates/ui-egui/src/main.rs", source);
            assert!(!ui_has_no_side_effects(d.path()), "{source}");
        }
    }

    #[test]
    fn cfg_test_methods_do_not_exempt_production_siblings() {
        let d = fixture(
            "crates/ui-egui/src/lib.rs",
            r#"
            struct Screen;
            impl Screen {
                #[cfg(all(unix, test))]
                fn test_only() { std::process::Command::new("test"); std::fs::write("out", b"x"); }
                fn production() { let _ = std::fs::read("in"); }
            }
        "#,
        );
        assert!(process_calls_only_in_platform(d.path()));
        assert!(ui_has_no_side_effects(d.path()));
        std::fs::write(
            d.path().join("crates/ui-egui/src/lib.rs"),
            r#"
            struct Screen; impl Screen {
                #[cfg(test)] fn test_only() {}
                fn production() { std::process::Command::new("app"); std::fs::write("out", b"x"); }
            }
        "#,
        )
        .unwrap();
        assert!(!process_calls_only_in_platform(d.path()));
        assert!(!ui_has_no_side_effects(d.path()));
    }

    #[test]
    fn production_source_directories_are_not_test_or_platform_exemptions() {
        for name in ["tests", "platform", "xtask", "target", "examples"] {
            let d = fixture("crates/sample/src/lib.rs", format!("mod {name};"));
            let dir = d.path().join("crates/sample/src").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("mod.rs"),
                r#"
                fn f() { std::process::Command::new("app"); let n = e.session().a() + e.session().b(); }
            "#,
            )
            .unwrap();
            assert!(!process_calls_only_in_platform(d.path()), "production src/{name}");
            assert!(!locks::check(d.path()), "production src/{name}");
            std::fs::write(d.path().join("crates/sample/src/lib.rs"), format!("#[cfg(test)] mod {name};")).unwrap();
            assert!(process_calls_only_in_platform(d.path()), "test-only src/{name}");
            assert!(locks::check(d.path()), "test-only src/{name}");
        }
        let d = fixture("crates/ui-egui/src/lib.rs", "mod tests;");
        std::fs::create_dir_all(d.path().join("crates/ui-egui/src/tests")).unwrap();
        std::fs::write(d.path().join("crates/ui-egui/src/tests/mod.rs"), "fn f() { std::fs::write(\"out\", b\"x\"); }").unwrap();
        assert!(!ui_has_no_side_effects(d.path()));
        std::fs::write(d.path().join("crates/ui-egui/src/lib.rs"), "#[cfg(test)] mod tests;").unwrap();
        assert!(ui_has_no_side_effects(d.path()));
    }

    #[test]
    fn explicit_modules_outside_src_do_not_gain_name_based_exemptions() {
        let d = fixture("crates/sample/src/lib.rs", "#[path = \"../modules/platform/runtime.rs\"] mod runtime;");
        let dir = d.path().join("crates/sample/modules/platform");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("runtime.rs"), "fn f() { std::process::Command::new(\"app\"); }").unwrap();
        assert!(!process_calls_only_in_platform(d.path()));
        std::fs::write(d.path().join("crates/sample/src/lib.rs"), "#[cfg(test)] #[path = \"../modules/platform/runtime.rs\"] mod runtime;").unwrap();
        assert!(process_calls_only_in_platform(d.path()));
    }

    #[test]
    fn hidden_module_directories_stay_in_source_scope() {
        for relative in [".internal/helper.rs", "../.internal/helper.rs"] {
            let declaration = format!("#[path = {relative:?}] mod helper;");
            let d = fixture("crates/ui-egui/src/lib.rs", &declaration);
            let source = d.path().join("crates/ui-egui/src").join(relative);
            std::fs::create_dir_all(source.parent().unwrap()).unwrap();
            std::fs::write(&source, "fn f() { std::process::Command::new(\"app\"); std::fs::write(\"out\", b\"x\"); let _ = Session::default(); let n = e.session().a() + e.session().b(); }").unwrap();
            assert!(!process_calls_only_in_platform(d.path()), "{relative}");
            assert!(!ui_has_no_side_effects(d.path()), "{relative}");
            assert!(!no_real_machine_sessions(d.path()), "{relative}");
            assert!(!locks::check(d.path()), "{relative}");
            std::fs::write(&source, "fn allowed() {}").unwrap();
            assert!(process_calls_only_in_platform(d.path()));
            assert!(ui_has_no_side_effects(d.path()));
            assert!(no_real_machine_sessions(d.path()));
            assert!(locks::check(d.path()));
            std::fs::write(d.path().join("crates/ui-egui/src/lib.rs"), format!("#[cfg(test)] {declaration}")).unwrap();
            std::fs::write(
                &source,
                "fn f() { std::process::Command::new(\"test\"); std::fs::write(\"out\", b\"x\"); let n = e.session().a() + e.session().b(); }",
            )
            .unwrap();
            assert!(process_calls_only_in_platform(d.path()));
            assert!(ui_has_no_side_effects(d.path()));
            assert!(locks::check(d.path()));
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_source_directories_fail_visibly_and_file_links_are_scanned() {
        use std::os::unix::fs::symlink;
        let d = fixture("crates/ui-egui/src/lib.rs", "#[path = \"shared/helper.rs\"] mod helper;");
        let shared = d.path().join("shared-source");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::write(shared.join("helper.rs"), "fn f() { std::process::Command::new(\"app\"); }").unwrap();
        symlink(&shared, d.path().join("crates/ui-egui/src/shared")).unwrap();
        let error = source_policy::rust_files(&d.path().join("crates"), &[]).unwrap_err();
        assert!(error.contains("symlinked source directory"), "{error}");
        assert!(!process_calls_only_in_platform(d.path()));
        assert!(!ui_has_no_side_effects(d.path()));
        assert!(!no_real_machine_sessions(d.path()));
        assert!(!locks::check(d.path()));

        let broken = fixture("crates/ui-egui/src/lib.rs", "#[path = \"shared/helper.rs\"] mod helper;");
        symlink(broken.path().join("missing"), broken.path().join("crates/ui-egui/src/shared")).unwrap();
        assert!(source_policy::rust_files(&broken.path().join("crates"), &[]).is_err(), "an uninspectable link cannot be counted clean");

        let d = fixture("crates/ui-egui/src/lib.rs", "mod shared;");
        let source = d.path().join("shared.rs");
        std::fs::write(&source, "fn allowed() {}").unwrap();
        symlink(&source, d.path().join("crates/ui-egui/src/shared.rs")).unwrap();
        assert!(process_calls_only_in_platform(d.path()));
        assert!(ui_has_no_side_effects(d.path()));
        assert!(no_real_machine_sessions(d.path()));
        assert!(locks::check(d.path()));
        std::fs::write(&source, "fn f() { std::process::Command::new(\"app\"); }").unwrap();
        assert!(!process_calls_only_in_platform(d.path()), "a linked Rust file is still checked");
    }

    #[test]
    fn explicit_production_modules_override_directory_exemptions() {
        let d = fixture("crates/ui-egui/src/lib.rs", "#[path = \"../tests/runtime.rs\"] mod runtime;");
        let dir = d.path().join("crates/ui-egui/tests");
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("runtime.rs");
        std::fs::write(&source, "fn f() { std::process::Command::new(\"app\"); std::fs::write(\"out\", b\"x\"); let n = e.session().a() + e.session().b(); }")
            .unwrap();
        assert!(!process_calls_only_in_platform(d.path()));
        assert!(!ui_has_no_side_effects(d.path()));
        assert!(!locks::check(d.path()));
        std::fs::write(d.path().join("crates/ui-egui/src/lib.rs"), "#[cfg(test)] #[path = \"../tests/runtime.rs\"] mod runtime;").unwrap();
        assert!(process_calls_only_in_platform(d.path()));
        assert!(ui_has_no_side_effects(d.path()));
        assert!(locks::check(d.path()));
        std::fs::write(d.path().join("crates/ui-egui/src/lib.rs"), "fn allowed() {}").unwrap();
        assert!(process_calls_only_in_platform(d.path()), "unreferenced integration tests stay exempt");
        assert!(locks::check(d.path()));
        std::fs::write(d.path().join("crates/ui-egui/src/lib.rs"), "#[path = \"../tests/runtime.rs\"] mod runtime;").unwrap();
        std::fs::write(&source, [0xff]).unwrap();
        assert!(!process_calls_only_in_platform(d.path()));
        assert!(!ui_has_no_side_effects(d.path()));
        assert!(!no_real_machine_sessions(d.path()));
        assert!(!locks::check(d.path()));
    }

    #[test]
    fn referenced_source_discovery_stays_within_the_workspace() {
        let outside = tempfile::tempdir().unwrap();
        let source = outside.path().join("module.rs");
        std::fs::write(&source, "fn allowed() {}").unwrap();
        let d = fixture("crates/ui-egui/src/lib.rs", format!("#[path = {source:?}] mod external;"));
        let files = source_policy::rust_files(&d.path().join("crates"), &[]).unwrap();
        let error = source_policy::read_all(files, d.path()).err().expect("external sources are unavailable");
        assert!(error.contains("outside the workspace"), "{error}");
        assert!(!process_calls_only_in_platform(d.path()));
        assert!(!ui_has_no_side_effects(d.path()));
        assert!(!no_real_machine_sessions(d.path()));
        assert!(!locks::check(d.path()));
        // A missing file under an inactive cfg is legal; the compiler handles active missing
        // modules rather than this syntax check guessing the full build configuration.
        std::fs::write(d.path().join("crates/ui-egui/src/lib.rs"), "#[cfg(any())] mod absent; fn allowed() {}").unwrap();
        assert!(process_calls_only_in_platform(d.path()));
        assert!(ui_has_no_side_effects(d.path()));
        assert!(no_real_machine_sessions(d.path()));
        assert!(locks::check(d.path()));
    }

    #[test]
    fn source_policies_fail_on_unreadable_rust() {
        let d = fixture("crates/ui-egui/src/main.rs", [0xff]);
        assert!(!process_calls_only_in_platform(d.path()));
        assert!(!ui_has_no_side_effects(d.path()));
        assert!(!no_real_machine_sessions(d.path()));
        assert!(!locks::check(d.path()));
    }

    #[test]
    fn out_of_line_test_modules_inherit_their_actual_scope() {
        let d = fixture("crates/sample/src/lib.rs", "#[cfg(all(test, unix))] mod tests; fn production() {}");
        std::fs::write(d.path().join("crates/sample/src/tests.rs"), "fn f() { std::process::Command::new(\"test\"); }").unwrap();
        assert!(process_calls_only_in_platform(d.path()));
        std::fs::write(d.path().join("crates/sample/src/lib.rs"), "mod tests;").unwrap();
        assert!(!process_calls_only_in_platform(d.path()), "a production module named tests is not exempt");
        std::fs::write(d.path().join("crates/sample/src/lib.rs"), "#[cfg(test)] #[path = \"tests.rs\"] mod checks; #[path = \"tests.rs\"] mod production;")
            .unwrap();
        assert!(!process_calls_only_in_platform(d.path()), "a shared source is still production");
    }

    #[test]
    fn a_test_module_reference_cannot_exempt_a_crate_root() {
        let d = fixture(
            "crates/sample/src/lib.rs",
            r#"
            #[cfg(test)] #[path = "lib.rs"] mod test_copy;
            fn production() { std::process::Command::new("app"); }
        "#,
        );
        assert!(!process_calls_only_in_platform(d.path()));
    }

    #[test]
    fn locks_check_a_production_file_even_when_named_tests() {
        let d = fixture("crates/sample/src/lib.rs", "mod tests;");
        std::fs::write(d.path().join("crates/sample/src/tests.rs"), "fn f() { let n = e.session().a() + e.session().b(); }").unwrap();
        assert!(!locks::check(d.path()));
        std::fs::write(d.path().join("crates/sample/src/lib.rs"), "#[cfg(test)] mod tests;").unwrap();
        assert!(locks::check(d.path()));
    }

    #[test]
    fn session_policy_checks_code_including_tests_but_not_examples() {
        let d = fixture(
            "crates/sample/src/lib.rs",
            r#"
            /* Session::default() */
            const EXAMPLE: &str = "Session::default()";
            fn production() { let _ = testing::session(); }
        "#,
        );
        assert!(no_real_machine_sessions(d.path()));
        let d = fixture("crates/sample/src/lib.rs", "#[cfg(test)] mod tests { fn f() { let _ = Session::default(); } }");
        assert!(!no_real_machine_sessions(d.path()));
    }

    /// `xtask` was the one member that didn't opt into the workspace lints, so the repo-wide
    /// `unsafe` ban didn't cover the tooling and a future `unwrap` in it would go unnoticed.
    #[test]
    fn every_member_manifest_uses_the_workspace_lints() {
        let root = workspace_root();
        let mut manifests = vec![root.join("xtask/Cargo.toml")];
        for group in ["crates", "apps"] {
            for entry in std::fs::read_dir(root.join(group)).unwrap().flatten() {
                let manifest = entry.path().join("Cargo.toml");
                if manifest.is_file() {
                    manifests.push(manifest);
                }
            }
        }
        assert!(manifests.len() > 10, "found the members: {manifests:?}");
        let missing: Vec<String> = manifests
            .iter()
            .filter(|m| !std::fs::read_to_string(m).unwrap_or_default().contains("[lints]\nworkspace = true"))
            .map(|m| m.strip_prefix(&root).unwrap_or(m).display().to_string())
            .collect();
        assert!(missing.is_empty(), "these members don't use [lints] workspace = true: {missing:?}");
    }
}
