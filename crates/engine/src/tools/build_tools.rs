//! A builder's own hands, for coding agents that don't bring their own (an API-key model driven
//! by our agent loop): list, read, write and edit files in the project it's building, and run its
//! build commands. They work in the session's workspace (the one a rebuild run attached), paths
//! relative to its folder. Writes and commands go through the same approval policy as Claude
//! Code's (`interaction_tools::approve`): edits inside app/ and the usual build commands go ahead,
//! dev servers are refused, anything else asks the person (or goes ahead in Auto mode, except
//! what's never done unasked). Ids follow docs/tool-ids.md.

use super::interaction_tools::{approve, inbox, inside, really_inside, resolve, workspace_of};
use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use crate::workspace::Workspace;
use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::process::Process;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Largest file one read returns (the rest is paged with offset/limit).
const MAX_READ: usize = 256 * 1024;
/// Largest file a write may create.
const MAX_WRITE: usize = 8 * 1024 * 1024;
/// Most entries one listing returns.
const MAX_ENTRIES: usize = 2_000;
/// Output kept from a command (its tail, where errors are).
const OUTPUT_TAIL: usize = 24 * 1024;

pub fn specs() -> Vec<ToolSpec> {
    let spec = |id, title, summary, beginner, effects, schema, run| ToolSpec {
        id,
        title,
        summary,
        beginner,
        toolset: Toolset::Build,
        effects,
        requires: Requires::Nothing,
        input_schema: schema,
        run,
    };
    vec![
        spec(
            "build_files_list",
            "List project files",
            "List files and folders in the app: app/ usually, or the project folder when the app is edited in place. A few levels deep; paths are relative to the project folder; build output and dependencies (node_modules, target, vendor, dist…) are skipped.",
            "Shows the files being built.",
            Effects::READ_ONLY,
            schema_of::<ListParams>,
            build_files_list,
        ),
        spec(
            "build_file_read",
            "Read a project file",
            "Read a text file in the project (app/, capture/, reference/…), paged by lines with offset/limit for big files.",
            "Reads one of the project's files.",
            Effects::READ_ONLY,
            schema_of::<ReadParams>,
            build_file_read,
        ),
        spec(
            "build_file_write",
            "Write a project file",
            "Create or replace a text file. Inside app/ this goes ahead; anywhere else asks the person.",
            "Writes one of the project's files.",
            Effects::WRITES_FILESYSTEM,
            schema_of::<WriteParams>,
            build_file_write,
        ),
        spec(
            "build_file_edit",
            "Edit a project file",
            "Replace exact text in a file: old_text must appear exactly once (or set replace_all). Inside app/ this goes ahead; anywhere else asks the person.",
            "Changes part of one of the project's files.",
            Effects::WRITES_FILESYSTEM,
            schema_of::<EditParams>,
            build_file_edit,
        ),
        spec(
            "build_checklist",
            "Update the checklist",
            "Show the person your plan as a short checklist and keep it current: todos with content (a plain step), activeForm (\"Drawing the map\") and status (pending, in_progress or completed). Call it again whenever a step changes; the app pins it above the prompt box.",
            "Shows the builder's plan as a checklist.",
            Effects::CHANGES_UI_STATE,
            schema_of::<ChecklistParams>,
            build_checklist,
        ),
        spec(
            "build_command_run",
            "Run a build command",
            "Run a command in app/ (npm install, npm run build, cargo build, the game engine's build…) and return its exit code and the end of its output. Common build commands go ahead; dev servers and watchers are refused (they never exit); anything else asks the person. Default limit 10 minutes, at most 2 hours.",
            "Runs a build step.",
            Effects::LAUNCHES_PROCESS.union(Effects::WRITES_FILESYSTEM).union(Effects::EXECUTES_FOREIGN_CODE),
            schema_of::<CommandParams>,
            build_command_run,
        ),
    ]
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    /// A folder relative to the project (default: the app folder; for a project edited in
    /// place, the project folder itself).
    #[serde(default)]
    pub path: Option<String>,
    /// How many levels down (default 4, at most 8).
    #[serde(default)]
    pub depth: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ReadParams {
    /// A file relative to the project, e.g. `app/src/main.rs` or `capture/README.md`.
    pub path: String,
    /// First line to return (0-based).
    #[serde(default)]
    pub offset: Option<usize>,
    /// How many lines (default: up to 256 KB).
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WriteParams {
    /// A file relative to the project, e.g. `app/src/main.rs`.
    pub path: String,
    /// The whole new contents.
    pub content: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditParams {
    /// A file relative to the project.
    pub path: String,
    /// The exact text to replace.
    pub old_text: String,
    /// What replaces it.
    pub new_text: String,
    /// Replace every occurrence instead of exactly one.
    #[serde(default)]
    pub replace_all: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ChecklistParams {
    /// The steps, in order.
    pub todos: Vec<Todo>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Todo {
    /// The step, in plain words.
    pub content: String,
    /// What it says while doing it ("Drawing the map").
    #[serde(default, rename = "activeForm")]
    pub active_form: Option<String>,
    /// `pending`, `in_progress` or `completed`.
    pub status: String,
}

/// The checklist is shown from the call itself (the run folds it into the project's feed); this
/// only checks it and says how far along it is.
fn build_checklist(_ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ChecklistParams = parse_params(params)?;
    if p.todos.is_empty() || p.todos.len() > 30 {
        return Err(ToolError::invalid("Give 1 to 30 steps."));
    }
    if let Some(bad) = p.todos.iter().find(|t| !["pending", "in_progress", "completed"].contains(&t.status.as_str())) {
        return Err(ToolError::invalid(format!("status {:?} must be pending, in_progress or completed", bad.status)));
    }
    let done = p.todos.iter().filter(|t| t.status == "completed").count();
    Ok(ToolOutput::Value(json!({"steps": p.todos.len(), "done": done})))
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CommandParams {
    /// The command, run in app/ (`&&` joins steps).
    pub command: String,
    /// Time limit in seconds (default 600, at most 7200).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
}

/// The workspace this session is building, from the inbox a rebuild run attached.
fn workspace(ctx: &Ctx<'_>) -> Result<Workspace, ToolError> {
    inbox(ctx).as_ref().and_then(workspace_of).ok_or_else(|| {
        ToolError::new(ErrorCode::FeatureMissing, "These tools work only while a project is being built.")
            .with_remediation("Start a build (rebuild_start or the app), then use them from the builder.")
    })
}

/// `rel` inside the project folder, or an error (no escaping with `..`, an absolute path or a
/// link that leads out of the project).
/// Models often write the project's folders from the root (`/app/src/main.rs`): a path like that
/// whose first folder the project has is read inside the project, where it was meant (#24).
fn in_project(ws: &Workspace, rel: &str) -> Result<PathBuf, ToolError> {
    let from_root = rel.trim_start_matches(['/', '\\']);
    let names_a_folder = from_root != rel
        && from_root.split(['/', '\\']).next().is_some_and(|first| !first.is_empty() && (ws.root.join(first).is_dir() || ws.root.join(first) == ws.app_dir()));
    [Some(rel), names_a_folder.then_some(from_root)].into_iter().flatten().find_map(|r| resolve(&ws.root, r).filter(|p| really_inside(&ws.root, p))).ok_or_else(
        || ToolError::invalid(format!("{rel} is outside the project.")).with_remediation("Use a path relative to the project folder, like app/src/main.rs."),
    )
}

/// Resolve `rel` inside `ws`, falling back to `ws.app_dir()` when `rel` is missing at `ws.root`
/// and doesn't already start with `app/`. Commands via `build_command_run` execute inside `app/`,
/// so compiler diagnostics report paths relative to `app/` rather than the workspace root.
fn in_project_or_app(ws: &Workspace, rel: &str, for_write: bool) -> Result<PathBuf, ToolError> {
    let path = in_project(ws, rel)?;
    if ws.manifest.app_in_root || path.exists() || rel.starts_with(['/', '\\']) {
        return Ok(path);
    }
    let starts_with_app = inside(&ws.app_dir(), &path) || rel.split(['/', '\\']).find(|c| !c.is_empty() && *c != ".").is_none_or(|first| first == "app");
    if starts_with_app {
        return Ok(path);
    }
    let Some(app_path) = resolve(&ws.app_dir(), rel).filter(|p| really_inside(&ws.app_dir(), p) && really_inside(&ws.root, p)) else {
        return Ok(path);
    };
    if app_path.exists() {
        return Ok(app_path);
    }
    if for_write {
        let app_parent_exists = app_path.parent().is_some_and(|parent| parent != ws.app_dir() && parent.is_dir());
        let root_parent_exists = path.parent().is_some_and(|parent| parent != ws.root && parent.is_dir());
        if app_parent_exists && !root_parent_exists {
            return Ok(app_path);
        }
    }
    Ok(path)
}

fn shown(ws: &Workspace, p: &Path) -> String {
    let root = resolve(&ws.root, ".").unwrap_or_else(|| ws.root.clone());
    let rel = p.strip_prefix(&root).unwrap_or(p);
    if rel.as_os_str().is_empty() { ".".to_string() } else { rel.to_string_lossy().replace('\\', "/") }
}

fn refused(why: String) -> ToolError {
    ToolError::new(ErrorCode::ConsentRequired, why)
}

fn build_files_list(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ListParams = parse_params(params)?;
    let ws = workspace(ctx)?;
    let default = if ws.manifest.app_in_root { "." } else { "app" };
    let root = in_project_or_app(&ws, p.path.as_deref().unwrap_or(default), false)?;
    // Our records are hidden from a listing of the project, but a listing of one of them (the
    // builder is told to read capture/ and reference/) shows what is in it.
    let listing_own = ws.is_own_record(&root);
    let depth = p.depth.unwrap_or(4).min(8);
    const SKIP: [&str; 9] = ["node_modules", "target", "vendor", "dist", "build", ".git", "Library", "Temp", "Intermediate"];
    let mut entries = Vec::new();
    let mut stack = vec![(root.clone(), 0usize)];
    let mut truncated = false;
    while let Some((dir, d)) = stack.pop() {
        let mut items: Vec<_> = std::fs::read_dir(&dir).into_iter().flatten().flatten().collect();
        items.sort_by_key(|e| e.file_name());
        for e in items {
            if entries.len() >= MAX_ENTRIES {
                truncated = true;
                break;
            }
            let path = e.path();
            // Craft Anything's own records are not the app, even when the app is the project
            // folder; the brief names them and the builder must leave them alone.
            if !listing_own && ws.is_own_record(&path) {
                continue;
            }
            let name = e.file_name().to_string_lossy().into_owned();
            // A link is listed but never followed: it may lead out of the project.
            if e.file_type().is_ok_and(|t| t.is_symlink()) {
                entries.push(json!({"path": shown(&ws, &path), "link": true}));
            } else if path.is_dir() {
                entries.push(json!({"path": format!("{}/", shown(&ws, &path))}));
                if d + 1 < depth && !SKIP.contains(&name.as_str()) {
                    stack.push((path, d + 1));
                }
            } else {
                entries.push(json!({"path": shown(&ws, &path), "bytes": e.metadata().map(|m| m.len()).unwrap_or(0)}));
            }
        }
    }
    Ok(ToolOutput::Value(json!({"root": shown(&ws, &root), "entries": entries, "truncated": truncated})))
}

fn build_file_read(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ReadParams = parse_params(params)?;
    let ws = workspace(ctx)?;
    let path = in_project_or_app(&ws, &p.path, false)?;
    let bytes = std::fs::read(&path).map_err(|e| ToolError::io(path.display(), e))?;
    if bytes.iter().take(8000).any(|b| *b == 0) {
        return Err(ToolError::invalid(format!("{} is a binary file.", p.path)).with_remediation("Read text files; look at pictures with your own viewer."));
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let start = p.offset.unwrap_or(0).min(lines.len());
    let mut out = String::new();
    let mut n = 0usize;
    let mut cut = false;
    for line in lines.iter().skip(start) {
        if p.limit.is_some_and(|l| n >= l) || (n > 0 && out.len() + line.len() > MAX_READ) {
            break;
        }
        // A line longer than a page comes cut to one, so reading always moves on (#280).
        let part = line.get(..line.floor_char_boundary(MAX_READ)).unwrap_or(line);
        cut |= part.len() < line.len();
        out.push_str(part);
        out.push('\n');
        n += 1;
    }
    let mut v = json!({"path": shown(&ws, &path), "offset": start, "lines": n, "total_lines": lines.len(), "more": start + n < lines.len(), "text": out});
    if cut {
        v["note"] = json!(format!("Line {} is longer than {} KiB, so only its start is shown.", start + 1, MAX_READ / 1024));
    }
    Ok(ToolOutput::Value(v))
}

fn write_checked(ctx: &mut Ctx<'_>, path: &Path, content: &str) -> Result<(), ToolError> {
    if content.len() > MAX_WRITE {
        return Err(ToolError::invalid("That file is too big to write in one go (8 MB at most)."));
    }
    approve(ctx, "Write", &json!({"file_path": path.display().to_string()}))?.map_err(refused)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| ToolError::io(dir.display(), e))?;
    }
    crate::project::write_atomic(path, content.as_bytes())
}

fn build_file_write(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: WriteParams = parse_params(params)?;
    let ws = workspace(ctx)?;
    let path = in_project_or_app(&ws, &p.path, true)?;
    write_checked(ctx, &path, &p.content)?;
    let mut out = json!({"written": shown(&ws, &path), "bytes": p.content.len()});
    // A model that leaves out `app/` would otherwise build beside the app, where no check looks.
    if !inside(&ws.app_dir(), &path) {
        out["note"] = json!("This file is outside app/: the app's own files go in app/ (paths like app/package.json).");
    }
    Ok(ToolOutput::Value(out))
}

fn build_file_edit(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: EditParams = parse_params(params)?;
    let ws = workspace(ctx)?;
    let path = in_project_or_app(&ws, &p.path, false)?;
    let text = std::fs::read_to_string(&path).map_err(|e| ToolError::io(path.display(), e))?;
    let count = if p.old_text.is_empty() { 0 } else { text.matches(p.old_text.as_str()).count() };
    let new = match (count, p.replace_all) {
        (0, _) => return Err(ToolError::invalid(format!("old_text isn't in {}.", p.path)).with_remediation("Read the file again and copy the text exactly.")),
        (1, _) | (_, true) => text.replace(p.old_text.as_str(), &p.new_text),
        (n, false) => {
            return Err(ToolError::invalid(format!("old_text appears {n} times in {}.", p.path))
                .with_remediation("Include more surrounding text so it appears once, or set replace_all."));
        }
    };
    write_checked(ctx, &path, &new)?;
    Ok(ToolOutput::Value(json!({"edited": shown(&ws, &path), "replaced": if p.replace_all { count } else { 1 }})))
}

fn build_command_run(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: CommandParams = parse_params(params)?;
    let ws = workspace(ctx)?;
    approve(ctx, "Bash", &json!({"command": p.command}))?.map_err(refused)?;
    let app = ws.app_dir();
    std::fs::create_dir_all(&app).map_err(|e| ToolError::io(app.display(), e))?;
    let inner = app.join("app");
    let had_inner = inner.is_dir();
    let (shell, flag) = if cfg!(windows) { ("cmd", "/C") } else { ("sh", "-c") };
    let cancel = ctx.cancel.clone();
    ctx.report(None, format!("Running {}", p.command.chars().take(120).collect::<String>()));
    let out = Process::new(shell)
        .args([flag, p.command.as_str()])
        .cwd(&app)
        .timeout(Duration::from_secs(p.timeout_secs.unwrap_or(600).clamp(1, 7200)))
        .stdout_cap(4 * 1024 * 1024)
        .stderr_cap(4 * 1024 * 1024)
        .cancel_when(move || cancel.is_cancelled())
        .label("the build command")
        .run()?;
    let tail = |s: String| -> String {
        let skip = s.len().saturating_sub(OUTPUT_TAIL);
        let mut start = skip;
        while !s.is_char_boundary(start) {
            start += 1;
        }
        s.get(start..).unwrap_or_default().to_string()
    };
    let mut v = json!({
        "command": p.command,
        "ran_in": ws.app_place(),
        "exit_code": out.code,
        "succeeded": out.success,
        "stdout": tail(out.stdout_text()),
        "stderr": tail(out.stderr_text()),
    });
    // A model that reads file paths and commands from one base starts with `cd app`, or makes its
    // project with `cargo new app`, from inside app/.
    let first = p.command.split(['&', ';', '\n']).next().unwrap_or_default().trim().replace('\\', "/");
    if ["cd app", "cd app/", "cd ./app", "cd ./app/"].contains(&first.as_str()) && !inner.is_dir() {
        v["note"] = json!(format!("Commands already run in {}: leave out `cd app`.", ws.app_place()));
    } else if !had_inner && app != ws.root && ["Cargo.toml", "package.json"].iter().any(|m| inner.join(m).is_file()) {
        // Only a project: a folder named app is fine there (Next.js keeps its routes in one).
        v["note"] = json!(
            "This made a project in app/app/: commands already run in app/, so the app's project goes in app/ itself (app/Cargo.toml, app/package.json)."
        );
    }
    Ok(ToolOutput::Value(v))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inbox::InboxDir;
    use crate::workspace::{ExistingFiles, TargetRef};
    use crate::{CallOptions, Caller, Consent, Engine};
    use std::sync::Arc;

    fn building() -> (std::path::PathBuf, Workspace, Engine) {
        let mut s = crate::testing::session();
        let dir = s.config_dir();
        let ws = Workspace::create(&dir, "Snake", "A snake game", TargetRef::None).unwrap();
        s.ext.insert(Arc::new(InboxDir(ws.inbox_dir())));
        (dir, ws, Engine::new(s))
    }

    fn call(e: &Engine, id: &str, p: Value) -> Result<Value, ToolError> {
        e.call(id, p, CallOptions::new(Caller::Mcp).with_consent(Consent::Granted)).map(|o| o.to_json()["result"].clone())
    }

    #[test]
    fn a_builder_reads_writes_and_edits_inside_its_project() {
        let (_d, ws, e) = building();
        call(&e, "build_file_write", json!({"path": "app/src/main.rs", "content": "fn main() {\n    println!(\"hi\");\n}\n"})).unwrap();
        let r = call(&e, "build_file_read", json!({"path": "app/src/main.rs"})).unwrap();
        assert_eq!(r["total_lines"], 3);
        call(&e, "build_file_edit", json!({"path": "app/src/main.rs", "old_text": "\"hi\"", "new_text": "\"hello\""})).unwrap();
        assert!(std::fs::read_to_string(ws.app_dir().join("src/main.rs")).unwrap().contains("hello"));
        let list = call(&e, "build_files_list", json!({})).unwrap();
        assert_eq!(list["root"], "app");
        assert!(list["entries"].to_string().contains("app/src/main.rs"), "{list}");
        let page = call(&e, "build_file_read", json!({"path": "app/src/main.rs", "offset": 1, "limit": 1})).unwrap();
        assert_eq!(page["text"], "    println!(\"hello\");\n");
        assert_eq!(page["more"], true);
        // Not found, ambiguous, outside the project.
        assert!(call(&e, "build_file_edit", json!({"path": "app/src/main.rs", "old_text": "nope", "new_text": "x"})).is_err());
        assert!(call(&e, "build_file_read", json!({"path": "../../etc/passwd"})).is_err());
        assert!(call(&e, "build_file_write", json!({"path": "../outside.txt", "content": "x"})).is_err());
    }

    /// "Work on my project" keeps the app in the project root: the default listing must be that
    /// root, not a missing or nested `app/`, and not Craft Anything's own records.
    #[test]
    fn a_project_edited_in_place_is_listed_from_its_own_folder() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("main.py"), "print('hi')\n").unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/app.py"), "x = 1\n").unwrap();
        // A nested app/ would be the wrong default: the app itself is the root.
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(d.path().join("app/other.txt"), "not the app\n").unwrap();
        let mut s = crate::testing::session();
        let ws = Workspace::create_at(d.path(), "Mine", "Add a page", TargetRef::None, Some(ExistingFiles::Edit)).unwrap();
        // checks/ isn't a Craft Anything record the builder must avoid: it keeps its own notes there.
        std::fs::create_dir_all(d.path().join("checks")).unwrap();
        std::fs::write(d.path().join("checks/notes.md"), "note\n").unwrap();
        // capture/ is an own record in the root listing, but the brief tells the builder to read it.
        std::fs::create_dir_all(d.path().join("capture/pages")).unwrap();
        std::fs::write(d.path().join("capture/README.md"), "evidence\n").unwrap();
        std::fs::write(d.path().join("capture/pages/index.html"), "<p>evidence</p>\n").unwrap();
        // A dependency tree bigger than the entry cap must not crowd out the app's own files.
        std::fs::create_dir_all(d.path().join("vendor")).unwrap();
        for i in 0..MAX_ENTRIES + 10 {
            std::fs::write(d.path().join("vendor").join(format!("f{i:04}.txt")), b"x").unwrap();
        }
        s.ext.insert(Arc::new(InboxDir(ws.inbox_dir())));
        let e = Engine::new(s);

        let list = call(&e, "build_files_list", json!({})).unwrap();
        assert_eq!(list["root"], ".");
        let entries = list["entries"].to_string();
        assert!(entries.contains("main.py") && entries.contains("src/app.py"), "{list}");
        assert!(entries.contains("app/other.txt"), "a real nested folder is still part of the root: {list}");
        assert!(entries.contains("checks/notes.md"), "the builder's own check notes stay visible: {list}");
        assert!(!entries.contains("craft.json"), "Craft Anything's own records are not app files: {list}");
        assert!(!entries.contains("capture/"), "capture stays out of the default root listing: {list}");
        assert!(entries.contains("vendor/"), "the dependency folder is listed but not walked: {list}");
        assert_eq!(list["truncated"], false, "vendor must not consume the entry cap: {list}");

        // The brief tells the builder to read capture/, so an explicit listing is allowed.
        let capture = call(&e, "build_files_list", json!({"path": "capture"})).unwrap();
        assert_eq!(capture["root"], "capture");
        assert!(capture["entries"].to_string().contains("capture/README.md"), "{capture}");
        assert!(capture["entries"].to_string().contains("capture/pages/"), "{capture}");
        let pages = call(&e, "build_files_list", json!({"path": "capture/pages"})).unwrap();
        assert_eq!(pages["root"], "capture/pages");
        assert!(pages["entries"].to_string().contains("capture/pages/index.html"), "{pages}");
        std::fs::create_dir_all(d.path().join("reference")).unwrap();
        std::fs::write(d.path().join("reference/look.png"), "x").unwrap();
        let reference = call(&e, "build_files_list", json!({"path": "reference"})).unwrap();
        assert!(reference["entries"].to_string().contains("reference/look.png"), "{reference}");

        // An explicit nested folder is still listed by that path.
        let nested = call(&e, "build_files_list", json!({"path": "app"})).unwrap();
        assert_eq!(nested["root"], "app");
        assert!(nested["entries"].to_string().contains("app/other.txt"), "{nested}");
    }

    /// A project path with `..` must not make listing paths absolute or hide an explicit capture.
    #[test]
    fn an_explicit_capture_listing_handles_a_project_path_with_dot_dot() {
        let base = tempfile::tempdir().unwrap();
        let project = base.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        let dotted = project.join("..").join("project");
        let mut s = crate::testing::session();
        let ws = Workspace::create_at(&dotted, "Mine", "Look at the evidence", TargetRef::None, None).unwrap();
        std::fs::create_dir_all(ws.capture_dir()).unwrap();
        std::fs::write(ws.capture_dir().join("README.md"), "evidence\n").unwrap();
        assert!(ws.is_own_record(&ws.capture_dir().join("README.md")), "capture is an own record");
        s.ext.insert(Arc::new(InboxDir(ws.inbox_dir())));
        let e = Engine::new(s);

        let capture = call(&e, "build_files_list", json!({"path": "capture"})).unwrap();
        assert_eq!(capture["root"], "capture");
        let entries = capture["entries"].to_string();
        assert!(entries.contains("capture/README.md"), "{capture}");
        let absolute = base.path().to_string_lossy().into_owned();
        assert!(!entries.contains(&absolute), "paths stay relative: {capture}");
    }

    /// A link to a folder; false where this system won't let a test make one.
    fn link_dir(target: &Path, link: &Path) -> bool {
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(target, link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(target, link);
        made.is_ok()
    }

    #[test]
    fn a_link_in_the_project_cannot_lead_outside_it() {
        let (_d, ws, e) = building();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "secret").unwrap();
        std::fs::create_dir_all(ws.app_dir().join("src")).unwrap();
        if !link_dir(outside.path(), &ws.app_dir().join("out")) {
            eprintln!("skipped: this system won't make links for tests");
            return;
        }
        assert!(call(&e, "build_file_read", json!({"path": "app/out/secret.txt"})).is_err());
        assert!(call(&e, "build_file_edit", json!({"path": "app/out/secret.txt", "old_text": "secret", "new_text": "x"})).is_err());
        assert!(call(&e, "build_file_write", json!({"path": "app/out/new.txt", "content": "x"})).is_err());
        assert!(call(&e, "build_file_write", json!({"path": "app/out/deeper/new.txt", "content": "x"})).is_err());
        assert_eq!(std::fs::read_to_string(outside.path().join("secret.txt")).unwrap(), "secret");
        assert!(!outside.path().join("new.txt").exists() && !outside.path().join("deeper").exists());
        // Listing shows the link but doesn't walk into it.
        let list = call(&e, "build_files_list", json!({})).unwrap();
        assert!(list["entries"].as_array().unwrap().iter().any(|x| x["path"] == "app/out" && x["link"] == true), "{list}");
        assert!(!list.to_string().contains("secret.txt"), "{list}");
        assert!(call(&e, "build_files_list", json!({"path": "app/out"})).is_err());
        // A link that stays inside the project still works.
        assert!(link_dir(&ws.app_dir().join("src"), &ws.app_dir().join("alias")));
        call(&e, "build_file_write", json!({"path": "app/alias/lib.rs", "content": "x"})).unwrap();
        assert!(ws.app_dir().join("src/lib.rs").is_file());
    }

    /// #280: a line longer than a page left reading stuck at it, with nothing shown.
    #[test]
    fn a_line_longer_than_a_page_is_shown_cut() {
        let (_d, _ws, e) = building();
        let long = "x".repeat(MAX_READ + 44_000);
        call(&e, "build_file_write", json!({"path": "app/min.js", "content": format!("{long}\nend\n")})).unwrap();
        let first = call(&e, "build_file_read", json!({"path": "app/min.js"})).unwrap();
        assert_eq!((first["lines"].as_u64(), first["more"].as_bool()), (Some(1), Some(true)));
        assert_eq!(first["text"].as_str().map(str::len), Some(MAX_READ + 1));
        assert!(first["note"].as_str().is_some_and(|n| n.contains("Line 1")), "{first}");
        let next = call(&e, "build_file_read", json!({"path": "app/min.js", "offset": 1})).unwrap();
        assert_eq!((next["text"].as_str(), next["more"].as_bool(), next.get("note")), (Some("end\n"), Some(false), None));
    }

    /// #24: a model that writes `/app/…` builds the app, and one that leaves out `app/` is told.
    #[test]
    fn paths_from_the_root_name_the_project_folders() {
        let (_d, ws, e) = building();
        let r = call(&e, "build_file_write", json!({"path": "/app/package.json", "content": "{}"})).unwrap();
        assert_eq!((r["written"].as_str(), r.get("note")), (Some("app/package.json"), None));
        assert!(ws.app_dir().join("package.json").is_file());
        // Backslashes separate folders only on Windows; elsewhere they're part of a name.
        let read = if cfg!(windows) { "\\app\\package.json" } else { "/app/package.json" };
        assert_eq!(call(&e, "build_file_read", json!({"path": read})).unwrap()["text"], "{}\n");
        assert!(call(&e, "build_files_list", json!({"path": "/app"})).unwrap()["entries"].to_string().contains("app/package.json"));
        // A folder the project doesn't have stays outside it.
        assert!(call(&e, "build_file_read", json!({"path": "/etc/passwd"})).is_err());
        assert!(call(&e, "build_file_write", json!({"path": "/nowhere/x.txt", "content": "x"})).is_err());
        assert!(call(&e, "build_file_write", json!({"path": "/../outside.txt", "content": "x"})).is_err());
        let stray = call(&e, "build_file_write", json!({"path": "index.html", "content": "<p>hi</p>"})).unwrap();
        assert!(stray["note"].as_str().unwrap().contains("outside app/"), "{stray}");
    }

    #[test]
    fn writes_outside_app_and_dev_servers_follow_the_policy() {
        let (_d, ws, e) = building();
        // Auto mode is on by default: outside app/ (but inside the project) goes ahead.
        call(&e, "build_file_write", json!({"path": "checks/notes.md", "content": "ok"})).unwrap();
        // The person's own records are never written unasked.
        assert!(call(&e, "build_file_write", json!({"path": "craft.json", "content": "{}"})).is_err());
        assert!(ws.root.join("craft.json").is_file(), "untouched");
        // Dev servers never exit: refused before anything runs.
        let err = call(&e, "build_command_run", json!({"command": "npm run dev"})).unwrap_err();
        assert!(err.message.contains("dev servers"), "{}", err.message);
    }

    /// R5.4: a local model ran `cd app && cargo build` from inside app/, reading file paths and
    /// commands from one base.
    #[test]
    fn a_command_says_where_it_ran() {
        let (_d, _ws, e) = building();
        let ok = call(&e, "build_command_run", json!({"command": "echo built"})).unwrap();
        assert_eq!((ok["ran_in"].as_str(), ok["succeeded"].as_bool(), ok.get("note")), (Some("app/"), Some(true), None));
        let lost = call(&e, "build_command_run", json!({"command": "cd app && echo built"})).unwrap();
        assert_eq!(lost["succeeded"], false, "{lost}");
        assert!(lost["note"].as_str().is_some_and(|n| n.contains("already run in app/")), "{lost}");
        let lines = call(&e, "build_command_run", json!({"command": "cd app\necho built"})).unwrap();
        assert!(lines["note"].as_str().is_some_and(|n| n.contains("already run in app/")), "{lines}");
        // Live, its first command was `cargo new app`, which made a project in app/app/.
        let nested = call(&e, "build_command_run", json!({"command": "mkdir app && cd app && echo [package]> Cargo.toml"})).unwrap();
        assert!(nested["note"].as_str().is_some_and(|n| n.contains("This made a project in app/app/")), "{nested}");
        // Once there, it's a folder of the app's own: going into it is left alone.
        let inner = call(&e, "build_command_run", json!({"command": "cd app && echo built"})).unwrap();
        assert_eq!((inner["succeeded"].as_bool(), inner.get("note")), (Some(true), None));
        // A folder named app with no project in it is fine (Next.js keeps its routes in one).
        let (_d, _ws, e) = building();
        let routes = call(&e, "build_command_run", json!({"command": "mkdir app"})).unwrap();
        assert_eq!(routes.get("note"), None, "{routes}");
    }

    #[test]
    fn the_checklist_is_checked() {
        let (_d, _ws, e) = building();
        let ok = call(&e, "build_checklist", json!({"todos": [{"content": "Draw the map", "activeForm": "Drawing the map", "status": "completed"}, {"content": "Add the snake", "status": "in_progress"}]})).unwrap();
        assert_eq!((ok["steps"].as_u64(), ok["done"].as_u64()), (Some(2), Some(1)));
        assert!(call(&e, "build_checklist", json!({"todos": [{"content": "x", "status": "maybe"}]})).is_err());
        assert!(call(&e, "build_checklist", json!({"todos": []})).is_err());
    }

    #[test]
    fn only_while_building() {
        let e = Engine::new(crate::testing::session());
        let err = call(&e, "build_files_list", json!({})).unwrap_err();
        assert_eq!(err.code, ErrorCode::FeatureMissing);
    }

    #[test]
    fn app_relative_paths_fall_back_to_app_dir() {
        let (_d, ws, e) = building();
        call(&e, "build_file_write", json!({"path": "app/crates/example/src/board.rs", "content": "pub struct Board;\n"})).unwrap();

        // Compiler errors from build_command_run emit paths relative to app/ (without app/ prefix).
        let r = call(&e, "build_file_read", json!({"path": "crates/example/src/board.rs"})).unwrap();
        assert_eq!(r["path"], "app/crates/example/src/board.rs");
        assert_eq!(r["text"], "pub struct Board;\n");

        // build_file_edit falls back to the existing file in app/.
        let ed = call(&e, "build_file_edit", json!({"path": "crates/example/src/board.rs", "old_text": "Board", "new_text": "CanvasBoard"})).unwrap();
        assert_eq!(ed["edited"], "app/crates/example/src/board.rs");
        assert!(std::fs::read_to_string(ws.app_dir().join("crates/example/src/board.rs")).unwrap().contains("CanvasBoard"));

        // build_files_list falls back when listing an app-relative subdirectory.
        let list = call(&e, "build_files_list", json!({"path": "crates/example"})).unwrap();
        assert_eq!(list["root"], "app/crates/example");
        assert!(list["entries"].to_string().contains("app/crates/example/src/board.rs"), "{list}");

        // build_file_write writes inside app/ when the parent folder already exists in app/ and not at root.
        let wr = call(&e, "build_file_write", json!({"path": "crates/example/src/new_mod.rs", "content": "pub struct NewMod;\n"})).unwrap();
        assert_eq!((wr["written"].as_str(), wr.get("note")), (Some("app/crates/example/src/new_mod.rs"), None));
        assert!(ws.app_dir().join("crates/example/src/new_mod.rs").is_file());
        assert!(!ws.root.join("crates/example/src/new_mod.rs").exists());

        // Overwriting an existing file via build_file_write without app/ also targets app/.
        let ov = call(&e, "build_file_write", json!({"path": "crates/example/src/board.rs", "content": "pub struct Updated;\n"})).unwrap();
        assert_eq!(ov["written"], "app/crates/example/src/board.rs");

        // Root-level stray file without an app-specific parent still writes to root with the note.
        let stray = call(&e, "build_file_write", json!({"path": "README.md", "content": "hi"})).unwrap();
        assert!(stray["note"].as_str().unwrap().contains("outside app/"), "{stray}");

        // Explicit app/ paths (including ./app/...) must never fall back into a nested app/app/.
        call(&e, "build_file_write", json!({"path": "app/app/page.tsx", "content": "nested"})).unwrap();
        let err_plain = call(&e, "build_file_read", json!({"path": "app/page.tsx"})).unwrap_err();
        let err_dot = call(&e, "build_file_read", json!({"path": "./app/page.tsx"})).unwrap_err();
        assert_eq!(err_plain.code, err_dot.code);
        let written = call(&e, "build_file_write", json!({"path": "./app/page.tsx", "content": "top"})).unwrap();
        assert_eq!(written["written"], "app/page.tsx");
        assert_eq!(std::fs::read_to_string(ws.app_dir().join("app/page.tsx")).unwrap(), "nested");
        assert_eq!(std::fs::read_to_string(ws.app_dir().join("page.tsx")).unwrap(), "top");
    }
}
