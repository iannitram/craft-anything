//! The stacks a build can use, from the `stacks.toml` table: what the builder is told for each,
//! and the phrases that ask for one by name. Kinds name their default stack; the table says how
//! to build with it. The table also lists libraries asked for on top of a stack (shadcn/ui,
//! Tailwind CSS), whose steps are added to the stack's.

use std::sync::OnceLock;

/// One stack ("React + TypeScript (Vite)", "Unity"…).
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Stack {
    /// Shown to the person and stored in the project.
    pub name: String,
    /// The game engine it builds with (`unity`, `unreal`, `godot`, `rust`), if any.
    #[serde(default)]
    pub engine: Option<String>,
    /// Phrases in a request that ask for this stack (`" in python"`), matched on the lower-case
    /// request padded with spaces.
    #[serde(default)]
    pub asked_by: Vec<String>,
    /// What the builder is told; `{editor}`, `{player}` and `{ext}` are filled in. `None` gets
    /// the generic instruction.
    #[serde(default)]
    pub steps: Option<String>,
    /// package.json dependencies that show a build uses this stack (`next`, `vite`); in a
    /// person's own project, Cargo.toml's too (`eframe`).
    #[serde(default)]
    pub uses: Vec<String>,
    /// What a person's own project must depend on as well to count as this stack (`react`).
    #[serde(default)]
    pub needs: Vec<String>,
    /// Files, relative to a folder, that show a person's own project is built with this stack
    /// (`project.godot`); a leading `*` matches by the name's ending (`*.uproject`).
    #[serde(default)]
    pub found_by: Vec<String>,
    /// Its build is a website: a project built with it and nothing to recreate is one.
    #[serde(default)]
    pub site: bool,
    /// The program its build can't run without (`cargo`), and how to get it.
    #[serde(default)]
    pub program: Option<String>,
    #[serde(default)]
    pub program_from: Option<String>,
}

pub use craft_anything_engine::toolchains::{Toolchain, found_in as toolchain_in};

/// The stack that leaves the choice to the builder (a website's default); [`used_in`] then
/// records the one it used.
pub const BUILDERS_CHOICE: &str = "Builder's choice";

/// A component or styling library asked for on top of a stack ("shadcn/ui", "Tailwind CSS").
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Library {
    pub name: String,
    /// Phrases in a request that ask for it, matched like [`Stack::asked_by`].
    #[serde(default)]
    pub asked_by: Vec<String>,
    /// The stacks it works with, by name; empty = any.
    #[serde(default)]
    pub stacks: Vec<String>,
    /// What the builder is told, after the stack's steps.
    pub steps: String,
}

#[derive(Debug, Default, serde::Deserialize)]
struct Table {
    #[serde(default)]
    stack: Vec<Stack>,
    #[serde(default)]
    library: Vec<Library>,
}

/// The built-in table. A table that doesn't parse (a broken edit) leaves it empty, so every
/// stack gets the generic instruction instead of the app failing; a test keeps it parsing.
fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| parse(include_str!("stacks.toml")).unwrap_or_default())
}

pub fn catalogue() -> &'static [Stack] {
    &table().stack
}

fn parse(text: &str) -> Result<Table, toml::de::Error> {
    toml::from_str::<Table>(text)
}

/// A request as `asked_by` phrases are matched: lower-case words, punctuation turned into
/// spaces, padded with spaces (`"Use Next.js, shadcn/ui"` → `" use next js shadcn ui "`).
pub fn words(request: &str) -> String {
    let lower = request.to_lowercase();
    format!(" {} ", lower.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" "))
}

pub fn get(name: &str) -> Option<&'static Stack> {
    catalogue().iter().find(|s| s.name == name)
}

/// The stack's own program (`cargo`) when `have` says this computer doesn't have it, and how to get it.
pub fn missing_program(stack: &str, have: impl Fn(&str) -> bool) -> Option<(&'static str, &'static str)> {
    let s = get(stack)?;
    let program = s.program.as_deref().filter(|p| !have(p))?;
    Some((program, s.program_from.as_deref().unwrap_or_default()))
}

/// A stack picked by name: its full name or its engine id, ignoring case (`unity`, `Godot`,
/// `React + TypeScript (Vite)`).
pub fn resolve(name: &str) -> Option<&'static Stack> {
    let n = name.trim();
    catalogue().iter().find(|s| s.name.eq_ignore_ascii_case(n) || s.engine.as_deref().is_some_and(|e| e.eq_ignore_ascii_case(n)))
}

/// The first stack whose phrases appear in `words` (a request through [`words`]).
pub fn asked_for(words: &str) -> Option<&'static Stack> {
    catalogue().iter().find(|s| s.asked_by.iter().any(|p| words.contains(p.as_str())))
}

/// The stack a build in `app` uses, from its package.json dependencies (the first stack in table
/// order whose `uses` it lists); None when it has none or names no stack in the table.
pub fn used_in(app: &std::path::Path) -> Option<&'static Stack> {
    let deps = craft_anything_engine::outputs::npm_dependencies(app);
    catalogue().iter().find(|s| s.uses.iter().any(|d| deps.contains(d)))
}

/// The stack a folder of the person's own files is built with: an engine's project file says
/// most, then the dependencies, then the other files that name a stack. None when nothing does.
pub fn found_in(dir: &std::path::Path) -> Option<&'static Stack> {
    let by_file = |engine: bool| catalogue().iter().filter(|s| s.engine.is_some() == engine).find(|s| s.found_by.iter().any(|f| has_file(dir, f)));
    let mut deps = craft_anything_engine::outputs::npm_dependencies(dir);
    deps.extend(craft_anything_engine::outputs::cargo_dependencies(dir));
    let by_deps = || catalogue().iter().find(|s| s.uses.iter().any(|d| deps.contains(d)) && s.needs.iter().all(|d| deps.contains(d)));
    by_file(true).or_else(by_deps).or_else(|| by_file(false))
}

fn has_file(dir: &std::path::Path, name: &str) -> bool {
    match name.strip_prefix('*') {
        Some(ending) => std::fs::read_dir(dir).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().ends_with(ending)),
        None => dir.join(name).is_file(),
    }
}

/// The libraries `request` asks for that work with `stack`, in table order.
pub fn libraries_for(request: &str, stack: &str) -> Vec<&'static Library> {
    let w = words(request);
    table()
        .library
        .iter()
        .filter(|l| l.asked_by.iter().any(|p| w.contains(p.as_str())) && (l.stacks.is_empty() || l.stacks.iter().any(|s| s == stack)))
        .collect()
}

/// What the builder is told for `stack`. `editor`: the game engine's editor for engine stacks.
pub fn steps(stack: &str, editor: Option<&std::path::Path>) -> String {
    let Some(text) = get(stack).and_then(|s| s.steps.as_deref()) else {
        return format!("Use {stack}. Create the project in app/ and make sure it builds or runs without errors.");
    };
    let (player, ext) = if cfg!(windows) {
        ("StandaloneWindows64", "exe")
    } else if cfg!(target_os = "macos") {
        ("StandaloneOSX", "app")
    } else {
        ("StandaloneLinux64", "x86_64")
    };
    let editor = editor.map(|e| e.display().to_string()).unwrap_or_default();
    text.replace("{editor}", &editor).replace("{player}", player).replace("{ext}", ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_says_what_it_is_built_with() {
        // Each case in a folder of its own.
        let found = |files: &[(&str, &str)]| {
            let d = tempfile::tempdir().unwrap();
            for (name, text) in files {
                let path = d.path().join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            found_in(d.path()).map(|s| s.name.as_str())
        };
        let vite = r#"{"dependencies":{"react":"19"},"devDependencies":{"vite":"7"}}"#;
        assert_eq!(found(&[("project.godot", "")]), Some("Godot"));
        assert_eq!(found(&[("ProjectSettings/ProjectVersion.txt", "")]), Some("Unity"));
        assert_eq!(found(&[("Game.uproject", "{}")]), Some("Unreal Engine"));
        assert_eq!(found(&[("project.godot", ""), ("package.json", vite)]), Some("Godot"), "an engine's file says most");
        assert_eq!(found(&[("package.json", vite), ("index.html", "")]), Some("React + TypeScript (Vite)"));
        assert_eq!(
            found(&[("package.json", r#"{"dependencies":{"vue":"3"},"devDependencies":{"vite":"7"}}"#)]),
            Some(BUILDERS_CHOICE),
            "Vite alone isn't React"
        );
        assert_eq!(found(&[("package.json", r#"{"dependencies":{"svelte":"5"}}"#), ("index.html", "")]), Some(BUILDERS_CHOICE));
        assert_eq!(found(&[("Cargo.toml", "[dependencies]\neframe = \"0.30\"\n")]), Some("Rust + egui (eframe)"));
        assert_eq!(found(&[("Cargo.toml", "[dependencies]\nmacroquad = \"0.4\"\n")]), Some("Rust + macroquad"));
        assert_eq!(found(&[("Cargo.toml", "[dependencies]\nmacroquad = \"0.4\"\negui = \"0.30\"\n")]), Some("Rust + macroquad"), "a game with an egui overlay");
        assert_eq!(found(&[("Cargo.toml", "[dependencies]\niced = \"0.13\"\n")]), Some(BUILDERS_CHOICE));
        assert_eq!(found(&[("Cargo.toml", "not [ toml")]), Some(BUILDERS_CHOICE));
        assert_eq!(found(&[("pyproject.toml", "")]), Some("Python"));
        assert_eq!(found(&[("requirements.txt", "")]), Some("Python"));
        assert_eq!(found(&[("go.mod", "")]), Some(BUILDERS_CHOICE));
        assert_eq!(found(&[("App.csproj", "")]), Some(BUILDERS_CHOICE));
        assert_eq!(found(&[("index.html", "")]), Some("Plain HTML and CSS"));
        assert_eq!(found(&[("notes.md", "")]), None);
    }

    /// Verify file-based toolchain detection and that every shared row has build/test commands.
    #[test]
    fn a_folder_says_how_it_builds_and_tests_itself() {
        let tool = |file: &str| {
            let d = tempfile::tempdir().unwrap();
            std::fs::write(d.path().join(file), "").unwrap();
            toolchain_in(d.path()).map(|t| t.name.as_str())
        };
        assert_eq!(tool("Cargo.toml"), Some("Rust"));
        assert_eq!(tool("package.json"), Some("Node.js"));
        assert_eq!(tool("requirements.txt"), Some("Python"));
        assert_eq!(tool("go.mod"), Some("Go"));
        assert_eq!(tool("App.csproj"), Some(".NET"));
        assert_eq!(tool("Shop.sln"), Some(".NET"));
        assert_eq!(tool("pom.xml"), Some("Maven"));
        assert_eq!(tool("build.gradle.kts"), Some("Gradle"));
        assert_eq!(tool("Makefile"), Some("Make"));
        assert_eq!(tool("notes.md"), None);
        for t in craft_anything_engine::toolchains::catalogue() {
            assert!(!t.found_by.is_empty() && !t.build.is_empty() && !t.test.is_empty(), "{}", t.name);
        }
    }

    #[test]
    fn the_table_parses_and_covers_every_engine() {
        let names: Vec<&str> = catalogue().iter().map(|s| s.name.as_str()).collect();
        assert!(names.len() >= 7, "{names:?}");
        for b in craft_anything_engines::game::Builder::ALL {
            assert!(get(b.stack()).is_some(), "no stack in stacks.toml for {}", b.stack());
        }
        assert_eq!(get("Unity").and_then(|s| s.engine.as_deref()), Some("unity"));
        assert!(parse("[[stack]]\nname = 3").is_err(), "a broken table is an error, not a panic");
        for l in &table().library {
            assert!(!l.steps.is_empty() && !l.asked_by.is_empty(), "{l:?}");
            assert!(l.stacks.iter().all(|s| get(s).is_some()), "{} names a stack that isn't in the table", l.name);
        }
    }

    #[test]
    fn a_build_left_to_the_builder_is_recorded_under_the_stack_it_used() {
        let d = tempfile::tempdir().unwrap();
        assert!(
            get(BUILDERS_CHOICE)
                .is_some_and(|s| s.steps.as_deref().is_some_and(|t| t.starts_with("Use what their request names; otherwise choose the stack yourself")))
        );
        assert!(used_in(d.path()).is_none(), "no package.json");
        std::fs::write(d.path().join("Cargo.toml"), "[dependencies]\neframe = \"0.30\"\n").unwrap();
        assert!(used_in(d.path()).is_none(), "Cargo.toml never replaces a stack the request named");
        std::fs::write(d.path().join("package.json"), "not json").unwrap();
        assert!(used_in(d.path()).is_none(), "a broken package.json is no stack, not a crash");
        std::fs::write(d.path().join("package.json"), r#"{"dependencies": {"next": "16", "react": "19"}, "devDependencies": {"vite": "7"}}"#).unwrap();
        assert_eq!(used_in(d.path()).map(|s| s.name.as_str()), Some("Next.js + TypeScript"), "table order decides");
        std::fs::write(d.path().join("package.json"), r#"{"devDependencies": {"vite": "7"}}"#).unwrap();
        assert_eq!(used_in(d.path()).map(|s| s.name.as_str()), Some("React + TypeScript (Vite)"));
        std::fs::write(d.path().join("package.json"), r#"{"dependencies": {"astro": "5"}}"#).unwrap();
        assert!(used_in(d.path()).is_none(), "a stack the table doesn't know stays the builder's choice");
        assert_eq!(libraries_for("use shadcn", BUILDERS_CHOICE).len(), 1, "a library works with the builder's choice too");
    }

    #[test]
    fn spelled_variants_of_a_stack_and_its_libraries() {
        let next = Some("Next.js + TypeScript");
        for r in ["use nextjs", "Use Next.js", "built with next js", "in Next.JS please", "(Next.js) with React", "use next and shadcn", "a site in next"] {
            assert_eq!(asked_for(&words(r)).map(|s| s.name.as_str()), next, "{r}");
        }
        assert_eq!(asked_for(&words("what to do next")), None, "\"next\" alone isn't a stack");
        assert_eq!(asked_for(&words("build it with Vite")).map(|s| s.name.as_str()), Some("React + TypeScript (Vite)"));
        let names = |r: &str, stack: &str| libraries_for(r, stack).iter().map(|l| l.name.as_str()).collect::<Vec<_>>();
        for r in ["use shadcn", "with shadcn/ui", "shadcn-ui components", "ShadCN"] {
            assert_eq!(names(r, "Next.js + TypeScript"), ["shadcn/ui"], "{r}");
        }
        assert_eq!(names("shadcn and tailwind", "React + TypeScript (Vite)"), ["shadcn/ui", "Tailwind CSS"]);
        assert_eq!(names("TailwindCSS", "Next.js + TypeScript"), ["Tailwind CSS"]);
        assert!(names("shadcn", "Python").is_empty(), "a library only joins the stacks it works with");
        assert!(names("a plain page", "Next.js + TypeScript").is_empty());
    }

    #[test]
    fn named_game_frameworks_do_not_select_an_editor() {
        for (request, name) in [("Make a game in Phaser with Vite", "Phaser"), ("Make a game using Bevy", "Bevy")] {
            let stack = asked_for(&words(request)).unwrap();
            assert_eq!(stack.name, name);
            assert!(stack.engine.is_none());
            assert_eq!(resolve(name), Some(stack));
        }
        assert!(asked_for(&words("a game with a phaser cannon")).is_none());
    }

    #[test]
    fn named_stacks_and_placeholders() {
        assert_eq!(asked_for(&words("A tool in Python.")).map(|s| s.name.as_str()), Some("Python"));
        assert_eq!(asked_for(&words("a page with react")).map(|s| s.name.as_str()), Some("React + TypeScript (Vite)"));
        assert_eq!(asked_for(&words("a notes app")), None);
        assert_eq!(resolve("godot").map(|s| s.name.as_str()), Some("Godot"));
        assert_eq!(resolve(" unreal ").map(|s| s.name.as_str()), Some("Unreal Engine"));
        assert_eq!(resolve("rust + EGUI (eframe)").map(|s| s.name.as_str()), Some("Rust + egui (eframe)"));
        assert!(resolve("cobol").is_none());
        let unity = steps("Unity", Some(std::path::Path::new("/ed/Unity")));
        assert!(unity.contains("the editor is /ed/Unity") && !unity.contains('{'), "{unity}");
        assert_eq!(steps("Elm", None), "Use Elm. Create the project in app/ and make sure it builds or runs without errors.");
    }
}
