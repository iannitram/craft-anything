//! Runtime observation of native apps: the accessibility (UI) tree of a running app, a short
//! scripted UI walkthrough, and native call tracing under a debugger.
//!
//! The UI tree is read on macOS (the accessibility APIs) and on Windows (UI Automation, through a
//! PowerShell script that ships with us); the walkthrough and call tracing need macOS and `lldb`. On
//! any other host they return a clear `wrong_os` result immediately — they never hang
//! waiting for something that cannot exist. The work goes through
//! `craft_anything_platform::process` with a deadline, closed stdin and capped output, like every
//! other external program we run.

use crate::tool::{Ctx, Effects, Requires, ToolOutput, ToolSpec, Toolset, parse_params, schema_of};
use craft_anything_evidence::{ErrorCode, ToolError};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;

pub fn specs() -> Vec<ToolSpec> {
    let runtime = Toolset::Runtime;
    // All three run or inspect a live process, so they execute foreign code and need consent.
    let effects = Effects::MUTATES_SESSION.union(Effects::LAUNCHES_PROCESS).union(Effects::CHANGES_UI_STATE).union(Effects::EXECUTES_FOREIGN_CODE);
    vec![
        ToolSpec {
            id: "runtime_ui_read",
            title: "Read an app's screen",
            summary: "Read the UI tree of a running app by its pid: every window's controls in order with their depth, role, title, identifier and value (macOS: the accessibility tree, optionally with a screenshot; Windows: UI Automation, also the class, position and size, and whether it's enabled; window_id picks one window by its handle). Password fields' values are never read; long values are cut at 500 characters. Other hosts get a clear wrong_os result.",
            beginner: "See the buttons, labels and fields of a running app.",
            toolset: runtime,
            effects,
            requires: Requires::Os(&["macos", "windows"]),
            input_schema: schema_of::<ObserveUiParams>,
            run: runtime_ui_read,
        },
        ToolSpec {
            id: "runtime_ui_walkthrough_record",
            title: "Drive an app's screen",
            summary: "Run a short scripted UI walkthrough against a running app by its pid and record its UI tree before and after the steps. Each step names an action and an element (its exact title or identifier). macOS: any accessibility action (e.g. AXPress). Windows (UI Automation): press (invoke, else toggle, select or expand), expand, collapse, toggle, select or focus; AXPress, AXShowMenu and AXRaise work too. Each step reports dispatched, element_not_found, not_supported, unknown_action or failed. Password fields' titles and values are never read. Other hosts get wrong_os.",
            beginner: "Click through an app step by step and record what the screen shows.",
            toolset: runtime,
            effects,
            requires: Requires::Os(&["macos", "windows"]),
            input_schema: schema_of::<CaptureUiParams>,
            run: runtime_ui_walkthrough_record,
        },
        ToolSpec {
            id: "apple_calls_observe",
            title: "Watch a Mac app's calls",
            summary: "Launch a macOS program under lldb, set breakpoints on named functions and ObjC selectors, and record each hit with its argument registers. Bounded by duration and event count. macOS only.",
            beginner: "Watch which functions a Mac app calls as it runs.",
            toolset: runtime,
            effects: effects.union(Effects::ACCESSES_NETWORK),
            requires: Requires::Os(&["macos"]),
            input_schema: schema_of::<ObserveCallsParams>,
            run: apple_calls_observe,
        },
    ]
}

/// The error returned on hosts that cannot run these tools.
#[cfg(not(target_os = "macos"))]
fn wrong_os(what: &str) -> ToolError {
    ToolError::new(ErrorCode::FeatureMissing, format!("{what} needs macOS and is not available on this host ({}).", craft_anything_evidence::host_triple()))
        .with_remediation("Run this on a macOS machine with the command-line developer tools installed.")
        .with_details(serde_json::json!({"reason": "wrong_os", "host": craft_anything_evidence::host_triple()}))
}

// --------------------------------------------------------------------------------------------
// Parameters (shared across hosts so the schema is identical everywhere)

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveUiParams {
    /// The process id of the running app.
    #[serde(default)]
    pub pid: Option<u32>,
    /// A specific window to restrict to (on Windows, its window handle).
    #[serde(default)]
    pub window_id: Option<u64>,
    /// Include a screenshot (PNG, base64) alongside the tree. macOS 14+: captures one on-screen
    /// window belonging to pid, restricted by window_id when supplied, at up to 2048 pixels
    /// per side. Without window_id, captures the first matching window. Needs existing Screen
    /// Recording permission; failures are explicit limitations and never open permission dialogs.
    #[serde(default)]
    pub screenshot: bool,
    /// Include the accessibility tree.
    #[serde(default = "default_true")]
    pub accessibility: bool,
    /// Largest number of accessibility nodes to return.
    #[serde(default = "default_max_nodes")]
    pub max_nodes: usize,
}

fn default_true() -> bool {
    true
}
fn default_max_nodes() -> usize {
    5000
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CaptureUiParams {
    #[serde(default)]
    pub pid: Option<u32>,
    /// Steps to perform, each an accessibility action such as press/focus on an element path.
    #[serde(default)]
    pub steps: Vec<UiStep>,
    #[serde(default = "default_max_nodes")]
    pub max_nodes: usize,
    /// Wall-clock budget in milliseconds.
    #[serde(default = "default_duration_ms")]
    pub duration_ms: u64,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UiStep {
    /// Accessibility action, e.g. `AXPress` (on Windows also press, expand, collapse, toggle,
    /// select or focus).
    pub action: String,
    /// Identifier or title path to the element the action targets.
    pub element: String,
}

fn default_duration_ms() -> u64 {
    10_000
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveCallsParams {
    /// Absolute path to the macOS program to launch under the debugger.
    pub executable: String,
    #[serde(default)]
    pub arguments: Vec<String>,
    #[serde(default)]
    pub working_directory: Option<String>,
    /// Breakpoints to set.
    pub breakpoints: Vec<Breakpoint>,
    /// How long to let it run, in milliseconds (100..=600000).
    #[serde(default = "default_duration_ms")]
    pub duration_ms: u64,
    /// Most breakpoint hits to record.
    #[serde(default = "default_max_events")]
    pub max_events: usize,
}

fn default_max_events() -> usize {
    1000
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Breakpoint {
    /// Break when a named function is entered.
    Function {
        name: String,
        #[serde(default)]
        module: Option<String>,
    },
    /// Break on an Objective-C method.
    ObjcMethod { class_name: String, selector: String },
}

// --------------------------------------------------------------------------------------------
// Dispatch (host-specific implementations below)

/// The UI tree of a running process's windows, as `runtime_ui_read` returns it (`nodes`,
/// `truncated`): for tools that start a program and read its screen before closing it.
pub(super) fn ui_tree(ctx: &mut Ctx<'_>, pid: u32, max_nodes: usize) -> Result<Value, ToolError> {
    #[cfg(target_os = "macos")]
    {
        let request =
            serde_json::json!({"command": "observe", "pid": pid, "window_id": null, "screenshot": false, "accessibility": true, "max_nodes": max_nodes});
        macos::run_helper(ctx, request, 10_000)
    }
    #[cfg(windows)]
    {
        windows::ui_tree(ctx, pid, None, max_nodes)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (ctx, pid, max_nodes);
        Err(wrong_os("Reading a program's UI tree"))
    }
}

fn runtime_ui_read(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ObserveUiParams = parse_params(params)?;
    #[cfg(target_os = "macos")]
    {
        macos::runtime_ui_read(ctx, p)
    }
    #[cfg(windows)]
    {
        windows::runtime_ui_read(ctx, p)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (ctx, p);
        Err(wrong_os("runtime_ui_read"))
    }
}

fn runtime_ui_walkthrough_record(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: CaptureUiParams = parse_params(params)?;
    #[cfg(target_os = "macos")]
    {
        macos::runtime_ui_walkthrough_record(ctx, p)
    }
    #[cfg(windows)]
    {
        windows::runtime_ui_walkthrough_record(ctx, p)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = (ctx, p);
        Err(wrong_os("runtime_ui_walkthrough_record"))
    }
}

fn apple_calls_observe(ctx: &mut Ctx<'_>, params: Value) -> Result<ToolOutput, ToolError> {
    let p: ObserveCallsParams = parse_params(params)?;
    if p.duration_ms < 100 || p.duration_ms > 600_000 {
        return Err(ToolError::invalid("duration_ms must be 100..=600000"));
    }
    if p.breakpoints.is_empty() {
        return Err(ToolError::invalid("at least one breakpoint is required"));
    }
    #[cfg(target_os = "macos")]
    {
        macos::apple_calls_observe(ctx, p)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = ctx;
        Err(wrong_os("apple_calls_observe"))
    }
}

/// What a breakpoint's command prints before the function's name; each such line is one hit.
const HIT_MARK: &str = "CRAFT_HIT";

/// The command file takes one command per line, so a line break in a name or argument would add a
/// command of the caller's choosing (lldb can run Python and shell commands).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn single_line(p: &ObserveCallsParams) -> Result<(), ToolError> {
    let bad = |s: &str| s.chars().any(char::is_control);
    let names = p.breakpoints.iter().flat_map(|b| match b {
        Breakpoint::Function { name, module } => vec![name.as_str(), module.as_deref().unwrap_or_default()],
        Breakpoint::ObjcMethod { class_name, selector } => vec![class_name.as_str(), selector.as_str()],
    });
    let text = names.chain(p.arguments.iter().map(String::as_str)).chain([p.executable.as_str()]);
    if text.into_iter().any(bad) {
        return Err(ToolError::invalid("The executable, arguments and breakpoint names must not contain line breaks or other control characters."));
    }
    Ok(())
}

/// A failed debugger run is not an observation: lldb exits non-zero when it rejects its command
/// file, and a run that never launched the program says nothing about what it calls (#251).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn lldb_failure(success: bool, code: Option<i32>, stderr: &str) -> Option<ToolError> {
    if success {
        return None;
    }
    let tail: String = stderr.chars().rev().take(600).collect::<Vec<_>>().into_iter().rev().collect();
    let how = code.map_or_else(|| "was stopped by the system".to_string(), |c| format!("exited with code {c}"));
    Some(
        ToolError::new(ErrorCode::ProgramFailed, format!("lldb {how} before the call observation finished, so nothing was observed. {}", tail.trim()))
            .with_remediation("Check that the program runs on its own and that lldb (Xcode's command-line tools) can debug it; a program with the hardened runtime needs `codesign` to allow debugging.")
            .with_details(serde_json::json!({"exit_code": code, "stderr_tail": tail})),
    )
}

/// Build an lldb Python command that sets breakpoints, runs, and prints each hit as JSON.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn lldb_script(p: &ObserveCallsParams) -> String {
    let mut setup = String::new();
    setup.push_str("settings set auto-confirm true\n");
    setup.push_str(&format!("target create \"{}\"\n", p.executable.replace('"', "\\\"")));
    for bp in &p.breakpoints {
        match bp {
            Breakpoint::Function { name, module } => match module {
                Some(m) => setup.push_str(&format!("breakpoint set --name \"{name}\" --shlib \"{m}\"\n")),
                None => setup.push_str(&format!("breakpoint set --name \"{name}\"\n")),
            },
            Breakpoint::ObjcMethod { class_name, selector } => {
                setup.push_str(&format!("breakpoint set --name \"-[{class_name} {selector}]\"\n"));
                setup.push_str(&format!("breakpoint set --name \"+[{class_name} {selector}]\"\n"));
            }
        }
    }
    // A breakpoint command on every breakpoint set above that prints the function and continues
    // (`return False` doesn't stop). lldb wraps the text in a function whose arguments include
    // `frame`, and a command line can't hold line breaks, so the statements are separated by `;`:
    // a literal backslash and `n` between them was a Python `SyntaxError` that stopped lldb before
    // it launched anything (#251).
    let breakpoints: usize = p.breakpoints.iter().map(|b| if matches!(b, Breakpoint::ObjcMethod { .. }) { 2 } else { 1 }).sum();
    let ids = (1..=breakpoints).map(|i| i.to_string()).collect::<Vec<_>>().join(" ");
    setup.push_str(&format!("breakpoint command add --script-type python -o 'print(\"{HIT_MARK}\", frame.GetFunctionName()); return False' {ids}\n"));
    let args = p.arguments.iter().map(|a| a.replace('"', "\\\"")).map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(" ");
    setup.push_str(&format!("process launch -- {args}\n"));
    setup
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use craft_anything_evidence::{Authority, Confidence, Coverage, Environment, EvidenceBuilder, Limitation, Subject, host_triple};
    use craft_anything_platform::process::{Process, StdinMode};
    use serde_json::json;
    use std::time::Duration;

    fn env(ctx: &Ctx<'_>, engine: &str) -> Environment {
        Environment { clean_room: ctx.session.clean_room(), engine: engine.into(), engine_version: None, host: host_triple(), language_id: None }
    }

    /// A private owner-only working directory for a run.
    fn private_root() -> Result<std::path::PathBuf, ToolError> {
        super::super::artifact_tools::fresh_temp_dir("craft-native-ui")
    }

    // ---- apple_calls_observe via lldb ----

    pub fn apple_calls_observe(ctx: &mut Ctx<'_>, p: ObserveCallsParams) -> Result<ToolOutput, ToolError> {
        let exe = std::path::Path::new(&p.executable);
        if !exe.is_absolute() {
            return Err(ToolError::invalid("executable must be an absolute path"));
        }
        let lldb = craft_anything_engines::locate("lldb", &ctx.session.settings.engines.tools, &ctx.session.settings.engine_store_dir()).ok_or_else(|| {
            ToolError::new(ErrorCode::EngineMissing, "lldb was not found.")
                .with_remediation("Install the macOS command-line developer tools (xcode-select --install).")
        })?;

        single_line(&p)?;
        let setup = lldb_script(&p);

        let root = private_root()?;
        let script_path = root.join("trace.lldb");
        craft_anything_platform::write_atomic(&script_path, setup.as_bytes())?;

        let ct = ctx.cancel.clone();
        let mut cmd = Process::new(&lldb)
            .args(["--batch", "--no-lldbinit", "--source"])
            .arg(&script_path)
            .stdin(StdinMode::Closed)
            .timeout(Duration::from_millis(p.duration_ms.saturating_add(60_000)))
            .stdout_cap(8 * 1024 * 1024)
            .cancel_when(move || ct.is_cancelled())
            .label("lldb");
        if let Some(wd) = &p.working_directory {
            cmd = cmd.cwd(wd);
        }
        let out = cmd.run();
        let _ = std::fs::remove_dir_all(&root);
        let out = out?;
        if let Some(e) = lldb_failure(out.success, out.code, &out.stderr_text()) {
            return Err(e);
        }

        let hits: Vec<Value> = out
            .stdout_text()
            .lines()
            .filter_map(|l| l.strip_prefix(HIT_MARK).and_then(|r| r.strip_prefix(' ')).map(|f| json!({"function": f.trim()})))
            .take(p.max_events)
            .collect();
        let truncated = hits.len() >= p.max_events;
        let summary = format!(
            "Observed {} call(s) under lldb while running {}.",
            hits.len(),
            exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        );
        let mut b = EvidenceBuilder::new(
            "apple_calls_observe",
            Subject { kind: "capture".into(), sha256: None, name: exe.file_name().map(|n| n.to_string_lossy().into_owned()) },
            env(ctx, "lldb"),
        )
        .params(json!({"executable_name": exe.file_name().map(|n| n.to_string_lossy().into_owned()), "breakpoint_count": p.breakpoints.len(), "duration_ms": p.duration_ms}))
        .result(json!({"hits": hits, "exit_code": out.code, "stderr_tail": out.stderr_text().chars().rev().take(2000).collect::<String>().chars().rev().collect::<String>()}))
        .summary(summary)
        .confidence(Confidence::Observed)
        .authority(Authority::OurRun)
        .limitation(Limitation::new("observational", "A trace shows one run; other inputs may call differently."));
        if truncated {
            b = b.coverage(Coverage {
                status: craft_anything_evidence::CoverageStatus::Partial,
                examined: Some(hits.len() as u64),
                total: None,
                next_offset: None,
            });
        }
        Ok(ToolOutput::evidence(b.build()?))
    }

    // ---- accessibility helper (runtime_ui_read / runtime_ui_walkthrough_record) ----

    /// The Swift source of our accessibility helper, compiled lazily on the host.
    const UI_HELPER_SWIFT: &str = include_str!("apple_runtime/native_ui_helper.swift");

    fn build_helper(root: &std::path::Path) -> Result<std::path::PathBuf, ToolError> {
        let xcrun = craft_anything_platform::which::which("xcrun").ok_or_else(|| {
            ToolError::new(ErrorCode::EngineMissing, "xcrun was not found.")
                .with_remediation("Install the macOS command-line developer tools (xcode-select --install).")
        })?;
        let src = root.join("native_ui_helper.swift");
        craft_anything_platform::write_atomic(&src, UI_HELPER_SWIFT.as_bytes())?;
        let bin = root.join("native_ui_helper");
        let cache = root.join("module-cache");
        Process::new(&xcrun)
            .args(["swiftc", "-O", "-module-cache-path"])
            .arg(&cache)
            .arg("-o")
            .arg(&bin)
            .arg(&src)
            .stdin(StdinMode::Closed)
            .timeout(Duration::from_secs(60))
            .label("swiftc")
            .run_checked()?;
        Ok(bin)
    }

    /// Turn the helper's own error object into a tool error. The helper exits 0 after reporting a
    /// refusal (such as a denied Accessibility permission), so an unchecked `nodes: []` would look
    /// like a successful read of an empty tree (#240).
    pub(super) fn helper_error(result: &Value) -> Option<ToolError> {
        let message = result.get("error")?.as_str()?;
        let reason = result.get("reason").and_then(Value::as_str).unwrap_or("helper_error");
        let error = if reason == "accessibility_not_granted" {
            ToolError::new(ErrorCode::NoPermission, "The app's screen was not read: this app does not have Accessibility permission.".to_string())
                .with_remediation("Allow the app running Craft Anything in System Settings → Privacy & Security → Accessibility, then run this again.")
        } else {
            ToolError::new(ErrorCode::Internal, format!("The accessibility helper refused the request: {message}"))
                .with_remediation("This is a bug in Craft Anything; report it with the details below.")
        };
        Some(error.with_details(json!({"reason": reason})))
    }

    pub(super) fn run_helper(ctx: &mut Ctx<'_>, request: Value, duration_ms: u64) -> Result<Value, ToolError> {
        let root = private_root()?;
        let helper = build_helper(&root).inspect_err(|_| {
            let _ = std::fs::remove_dir_all(&root);
        })?;
        let ct = ctx.cancel.clone();
        let out = Process::new(&helper)
            .stdin(StdinMode::Bytes(serde_json::to_vec(&request).unwrap_or_default()))
            .timeout(Duration::from_millis(duration_ms.saturating_add(10_000)))
            .stdout_cap(16 * 1024 * 1024)
            .cancel_when(move || ct.is_cancelled())
            .label("accessibility helper")
            .run_checked();
        let _ = std::fs::remove_dir_all(&root);
        let out = out?;
        let result: Value = serde_json::from_slice(&out.stdout)
            .map_err(|e| ToolError::new(ErrorCode::GarbledOutput, format!("accessibility helper output was not JSON: {e}")))?;
        match helper_error(&result) {
            Some(error) => Err(error),
            None => Ok(result),
        }
    }

    /// Observe a running macOS app through the native helper and return its UI evidence.
    /// Records the accessibility selection and reports a deliberately skipped tree in the
    /// summary; with accessibility disabled, the helper needs no Accessibility permission.
    pub fn runtime_ui_read(ctx: &mut Ctx<'_>, p: ObserveUiParams) -> Result<ToolOutput, ToolError> {
        let pid = p.pid.ok_or_else(|| ToolError::invalid("pid is required to read a running app's UI"))?;
        let request = json!({"command": "observe", "pid": pid, "window_id": p.window_id, "screenshot": p.screenshot, "accessibility": p.accessibility, "max_nodes": p.max_nodes});
        let result = run_helper(ctx, request, 10_000)?;
        let nodes = result.get("nodes").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        let summary = if p.accessibility {
            format!("Read {nodes} accessibility node(s) from pid {pid}.")
        } else {
            format!("Skipped the accessibility tree for pid {pid}, as requested.")
        };
        let truncated = result.get("truncated").and_then(Value::as_bool).unwrap_or(false);
        let screenshot_limitation = if p.screenshot && result.get("screenshot").is_none() {
            let error = result.get("screenshot_error");
            Some(Limitation::new(
                error.and_then(|e| e.get("code")).and_then(Value::as_str).unwrap_or("screenshot_unavailable"),
                error
                    .and_then(|e| e.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("The requested screenshot was not captured; check the helper error and retry."),
            ))
        } else {
            None
        };
        let mut b =
            EvidenceBuilder::new("runtime_ui_read", Subject { kind: "ui".into(), sha256: None, name: Some(format!("pid {pid}")) }, env(ctx, "accessibility"))
                .params(json!({"pid": pid, "window_id": p.window_id, "max_nodes": p.max_nodes, "screenshot": p.screenshot, "accessibility": p.accessibility}))
                .result(result)
                .summary(summary)
                .confidence(Confidence::Observed)
                .authority(Authority::OurRun);
        if truncated {
            b = b
                .limitation(Limitation::new("tree_truncated", "The accessibility tree reached the node or output limit; only a prefix is included."))
                .coverage(Coverage { status: craft_anything_evidence::CoverageStatus::Partial, ..Coverage::default() });
        }
        if let Some(limitation) = screenshot_limitation {
            b = b.limitation(limitation).coverage(Coverage { status: craft_anything_evidence::CoverageStatus::Partial, ..Coverage::default() });
        }
        Ok(ToolOutput::evidence(b.build()?))
    }

    pub fn runtime_ui_walkthrough_record(ctx: &mut Ctx<'_>, p: CaptureUiParams) -> Result<ToolOutput, ToolError> {
        let pid = p.pid.ok_or_else(|| ToolError::invalid("pid is required to drive a running app's UI"))?;
        let steps: Vec<Value> = p.steps.iter().map(|s| json!({"action": s.action, "element": s.element})).collect();
        let request = json!({"command": "walkthrough", "pid": pid, "steps": steps, "max_nodes": p.max_nodes});
        let result = run_helper(ctx, request, p.duration_ms)?;
        let b = EvidenceBuilder::new(
            "runtime_ui_walkthrough_record",
            Subject { kind: "ui".into(), sha256: None, name: Some(format!("pid {pid}")) },
            env(ctx, "accessibility"),
        )
        .params(json!({"pid": pid, "step_count": p.steps.len()}))
        .result(result)
        .summary(format!("Ran {} UI step(s) against pid {pid}.", p.steps.len()))
        .confidence(Confidence::Observed)
        .authority(Authority::OurRun);
        Ok(ToolOutput::evidence(b.build()?))
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use craft_anything_evidence::{Authority, Confidence, Environment, EvidenceBuilder, Limitation, Subject, host_triple};
    use craft_anything_platform::process::{Process, StdinMode};
    use serde_json::json;
    use std::time::Duration;

    /// Reads or drives one process's UI Automation tree; the request arrives as JSON on stdin.
    const UI_PS1: &str = include_str!("apple_runtime/native_ui_windows.ps1");

    fn env(ctx: &Ctx<'_>) -> Environment {
        Environment { clean_room: ctx.session.clean_room(), engine: "ui-automation".into(), engine_version: None, host: host_triple(), language_id: None }
    }

    /// Run the UI Automation script with `request` (see its header). An error when the process
    /// has no windows.
    fn run(ctx: &mut Ctx<'_>, request: &Value, limit: Duration) -> Result<Value, ToolError> {
        // -EncodedCommand takes the script as base64 of UTF-16LE, so nothing in it needs quoting.
        let utf16: Vec<u8> = UI_PS1.encode_utf16().flat_map(u16::to_le_bytes).collect();
        let windir = std::env::var_os("SystemRoot").map_or_else(|| std::path::PathBuf::from(r"C:\Windows"), std::path::PathBuf::from);
        let shell = windir.join(r"System32\WindowsPowerShell\v1.0\powershell.exe");
        let args = ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand"]
            .map(String::from)
            .into_iter()
            .chain([super::super::web_common::b64_encode(&utf16)]);
        let ct = ctx.cancel.clone();
        let out = Process::new(&shell)
            .args(args)
            .stdin(StdinMode::Bytes(serde_json::to_vec(request).unwrap_or_default()))
            .timeout(limit)
            .stdout_cap(16 << 20)
            .cancel_when(move || ct.is_cancelled())
            .label("UI Automation reader")
            .run_checked()?;
        let json = out.stdout.strip_prefix(b"\xEF\xBB\xBF".as_slice()).unwrap_or(&out.stdout);
        let result: Value = serde_json::from_slice(json)
            .map_err(|e| ToolError::new(ErrorCode::GarbledOutput, format!("the UI Automation reader's output was not JSON: {e}")))?;
        if result.get("windows").and_then(Value::as_u64) == Some(0) {
            let pid = request.get("pid").and_then(Value::as_u64).unwrap_or_default();
            let handle = if request.get("window_id").is_some_and(|w| !w.is_null()) { " with that handle" } else { "" };
            return Err(ToolError::new(ErrorCode::NotFound, format!("Process {pid} has no windows open{handle}."))
                .with_remediation("Pass the pid of a running app with a window on screen (Task Manager's Details tab shows it)."));
        }
        Ok(result)
    }

    /// The UI Automation tree of a process's windows (`max` elements at most; `window_id` picks one
    /// window by its handle). An error when it has no windows.
    pub fn ui_tree(ctx: &mut Ctx<'_>, pid: u32, window_id: Option<u64>, max: usize) -> Result<Value, ToolError> {
        ctx.report(None, format!("Reading the screen of process {pid}"));
        run(ctx, &json!({"command": "observe", "pid": pid, "window_id": window_id, "max_nodes": max}), Duration::from_secs(120))
    }

    fn control_view() -> Limitation {
        Limitation::new(
            "control_view",
            "UI Automation's control view: plain layout containers are left out, and an app that draws its own controls (many games) shows one pane.",
        )
    }

    pub fn runtime_ui_read(ctx: &mut Ctx<'_>, p: ObserveUiParams) -> Result<ToolOutput, ToolError> {
        let pid = p.pid.ok_or_else(|| ToolError::invalid("pid is required to read a running app's UI"))?;
        let result = ui_tree(ctx, pid, p.window_id, if p.accessibility { p.max_nodes } else { 0 })?;
        let nodes = result.get("nodes").and_then(Value::as_array).map(Vec::len).unwrap_or(0);
        let mut b = EvidenceBuilder::new("runtime_ui_read", Subject { kind: "ui".into(), sha256: None, name: Some(format!("pid {pid}")) }, env(ctx))
            .params(json!({"pid": pid, "max_nodes": p.max_nodes, "screenshot": p.screenshot, "window_id": p.window_id}))
            .result(result)
            .summary(format!("Read {nodes} UI element(s) from pid {pid}."))
            .confidence(Confidence::Observed)
            .authority(Authority::OurRun)
            .limitation(control_view());
        if p.screenshot {
            b = b.limitation(Limitation::new("no_screenshot", "No screenshot is taken on Windows here; native_window_capture takes one."));
        }
        Ok(ToolOutput::evidence(b.build()?))
    }

    pub fn runtime_ui_walkthrough_record(ctx: &mut Ctx<'_>, p: CaptureUiParams) -> Result<ToolOutput, ToolError> {
        let pid = p.pid.ok_or_else(|| ToolError::invalid("pid is required to drive a running app's UI"))?;
        let steps: Vec<Value> = p.steps.iter().map(|s| json!({"action": s.action, "element": s.element})).collect();
        ctx.report(None, format!("Running {} step(s) on process {pid}", steps.len()));
        let request = json!({"command": "walkthrough", "pid": pid, "steps": steps, "max_nodes": p.max_nodes});
        let result = run(ctx, &request, Duration::from_millis(p.duration_ms.saturating_add(10_000)))?;
        let done = result
            .get("steps")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|s| s.get("outcome").and_then(Value::as_str) == Some("dispatched"))
            .count();
        let b = EvidenceBuilder::new("runtime_ui_walkthrough_record", Subject { kind: "ui".into(), sha256: None, name: Some(format!("pid {pid}")) }, env(ctx))
            .params(json!({"pid": pid, "step_count": p.steps.len()}))
            .result(result)
            .summary(format!("Ran {done} of {} UI step(s) against pid {pid}.", p.steps.len()))
            .confidence(Confidence::Observed)
            .authority(Authority::OurRun)
            .limitation(control_view())
            .limitation(Limitation::new(
                "patterns_not_input",
                "Steps use the controls' own UI Automation actions (press, expand, toggle, select, focus), not the mouse or keyboard; a control without one reports not_supported.",
            ));
        Ok(ToolOutput::evidence(b.build()?))
    }
}

#[cfg(test)]
mod tests {
    use crate::{CallOptions, Caller, Consent, Engine};
    use serde_json::json;

    fn granted() -> CallOptions<'static> {
        CallOptions::new(Caller::Cli).with_consent(Consent::Granted)
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_on_this_host() {
        let e = Engine::new(crate::testing::session());
        let mut tools = vec![("apple_calls_observe", json!({"executable": "/bin/true", "breakpoints": [{"kind": "function", "name": "main"}]}))];
        if !cfg!(windows) {
            tools.push(("runtime_ui_read", json!({"pid": 1})));
            tools.push(("runtime_ui_walkthrough_record", json!({"pid": 1})));
        }
        for (id, params) in tools {
            let err = e.call(id, params, granted()).unwrap_err();
            assert_eq!(err.code, craft_anything_evidence::ErrorCode::FeatureMissing, "{id}");
            assert_eq!(err.details.as_ref().unwrap()["reason"], "wrong_os", "{id}");
            assert!(err.remediation.is_some());
            // The host check comes before consent, and capabilities report the tool unavailable.
            let err = e.call(id, json!({}), CallOptions::new(Caller::Mcp)).unwrap_err();
            assert_eq!(err.code, craft_anything_evidence::ErrorCode::FeatureMissing, "{id}");
            let spec = *e.registry().get(id).unwrap();
            assert!(!e.availability(&spec, &e.session()).available, "{id}");
        }
    }

    /// Exercise the real serializer with synthetic AX values and image data; no UI permissions.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_budgets_combined_tree_and_screenshot_output() {
        use craft_anything_platform::process::Process;
        use std::time::Duration;
        let root = tempfile::tempdir().unwrap();
        let source = include_str!("apple_runtime/native_ui_helper.swift").split("let input = FileHandle.standardInput").next().unwrap();
        let fixture = include_str!("apple_runtime/output_budget_test.swift");
        let src = root.path().join("budget.swift");
        let bin = root.path().join("budget");
        std::fs::write(&src, format!("{source}\n{fixture}")).unwrap();
        Process::new("xcrun")
            .args(["swiftc", "-Onone", "-module-cache-path"])
            .arg(root.path().join("module-cache"))
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .timeout(Duration::from_secs(60))
            .run_checked()
            .unwrap();
        Process::new(&bin).timeout(Duration::from_secs(30)).run_checked().unwrap();
    }

    /// Run the real Swift helper without reading app UI or requiring Accessibility permission.
    /// Both zero and positive node limits must return an empty, non-truncated tree and evidence
    /// that records the skipped read.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_can_skip_accessibility_without_reading_a_tree() {
        let e = Engine::new(crate::testing::session());
        for max_nodes in [0, 1] {
            let output =
                e.call("runtime_ui_read", json!({"pid": std::process::id(), "accessibility": false, "max_nodes": max_nodes}), granted()).unwrap().to_json();
            assert_eq!(output["result"]["nodes"], json!([]), "{output}");
            assert_eq!(output["result"]["truncated"], false, "{output}");
            assert_eq!(output["params"]["accessibility"], false, "{output}");
            assert!(output["result"].get("screenshot").is_none(), "{output}");
            assert!(output["result"].get("screenshot_error").is_none(), "{output}");
            assert!(output["summary"].as_str().unwrap().contains("Skipped the accessibility tree"), "{output}");
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_reports_a_requested_screenshot_that_cannot_be_captured() {
        let e = Engine::new(crate::testing::session());
        // This process has no windows. A deliberately impossible window id ensures the helper
        // can never capture anything, even on a machine with Screen Recording permission.
        let output = e
            .call("runtime_ui_read", json!({"pid": std::process::id(), "window_id": u64::MAX, "accessibility": false, "screenshot": true}), granted())
            .unwrap()
            .to_json();
        assert_eq!(output["result"]["nodes"], json!([]), "{output}");
        assert!(output["result"].get("screenshot").is_none(), "{output}");
        let error = &output["result"]["screenshot_error"];
        assert!(
            matches!(
                error["code"].as_str(),
                Some("screen_recording_denied" | "screenshot_window_not_found" | "screenshot_unsupported" | "screenshot_capture_failed")
            ),
            "{output}"
        );
        assert!(output["limitations"].as_array().unwrap().iter().any(|l| l["code"] == error["code"] && l["message"] == error["message"]), "{output}");
        assert_eq!(output["coverage"]["status"], "partial", "{output}");
        assert_eq!(output["params"]["window_id"], u64::MAX, "{output}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn calls_validate_before_dispatch() {
        let e = Engine::new(crate::testing::session());
        // Bad duration is rejected on every host, before any host check.
        let err = e
            .call("apple_calls_observe", json!({"executable": "/bin/true", "breakpoints": [{"kind": "function", "name": "main"}], "duration_ms": 1}), granted())
            .unwrap_err();
        assert_eq!(err.code, craft_anything_evidence::ErrorCode::BadInput);
    }

    /// Exercise the real debugger with a program we own. Running this needs the macOS developer
    /// tools and a host that permits lldb to launch a locally compiled executable.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "needs macOS developer tools and permission to launch the debugger"]
    fn lldb_records_our_probe_and_refuses_a_non_executable() {
        use craft_anything_evidence::ErrorCode;
        use craft_anything_platform::process::Process;
        use std::time::Duration;

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("probe.c");
        let executable = root.path().join("probe");
        std::fs::write(
            &source,
            r#"
                __attribute__((noinline)) static int alpha(void) { return 1; }
                __attribute__((noinline)) static int beta(void) { return 2; }

                int main(void) {
                    int first = alpha();
                    int second = beta();
                    return first + second == 3 ? 0 : 1;
                }
            "#,
        )
        .unwrap();
        Process::new("xcrun").args(["clang", "-O0", "-g"]).arg(&source).arg("-o").arg(&executable).timeout(Duration::from_secs(30)).run_checked().unwrap();

        let engine = Engine::new(crate::testing::live_session());
        let observe = |executable: &std::path::Path| {
            engine.call(
                "apple_calls_observe",
                json!({
                    "executable": executable,
                    "breakpoints": [
                        {"kind": "function", "name": "alpha"},
                        {"kind": "function", "name": "beta"}
                    ]
                }),
                granted(),
            )
        };
        let output = observe(&executable).unwrap().to_json();
        assert_eq!(output["result"]["hits"], json!([{"function": "alpha"}, {"function": "beta"}]), "{output}");
        assert_eq!(output["result"]["exit_code"], 0, "{output}");

        let error = observe(&source).unwrap_err();
        assert_eq!(error.code, ErrorCode::ProgramFailed, "{error:?}");
        assert!(error.remediation.is_some(), "{error:?}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_reads_the_ui_tree_of_a_process_with_windows() {
        let e = Engine::new(crate::testing::session());
        // This test process has no windows: a clear error, not an empty tree.
        let err = e.call("runtime_ui_read", json!({"pid": std::process::id()}), granted()).unwrap_err();
        assert_eq!(err.code, craft_anything_evidence::ErrorCode::NotFound, "{err:?}");
        assert!(err.message.contains("no windows"), "{}", err.message);
        // Reading another app's screen needs consent, like on macOS.
        let err = e.call("runtime_ui_read", json!({"pid": 1}), CallOptions::new(Caller::Mcp)).unwrap_err();
        assert_eq!(err.code, craft_anything_evidence::ErrorCode::ConsentRequired);
    }

    /// An off-screen WinForms window: a button that writes "hi there" into a box, a password box
    /// holding "hunter2" and a checkbox. Returns once its window can be read.
    #[cfg(windows)]
    fn test_window(e: &Engine) -> std::process::Child {
        let form = "Add-Type -AssemblyName System.Windows.Forms, System.Drawing; $f = New-Object Windows.Forms.Form; \
            $f.StartPosition = 'Manual'; $f.Location = New-Object Drawing.Point(-4000, -4000); $f.ShowInTaskbar = $false; \
            $b = New-Object Windows.Forms.Button; $b.Name = 'hiButton'; $b.Text = 'Say hi'; \
            $t = New-Object Windows.Forms.TextBox; $t.Name = 'box'; $t.Top = 40; $b.Add_Click({ $t.Text = 'hi there' }); \
            $pw = New-Object Windows.Forms.TextBox; $pw.UseSystemPasswordChar = $true; $pw.Text = 'hunter2'; $pw.Top = 70; \
            $c = New-Object Windows.Forms.CheckBox; $c.Text = 'Remember me'; $c.Top = 100; \
            $f.Controls.AddRange(@($b, $t, $pw, $c)); [void]$f.ShowDialog()";
        let child = std::process::Command::new("powershell").args(["-NoProfile", "-NonInteractive", "-Command", form]).spawn().unwrap();
        for _ in 0..40 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if e.call("runtime_ui_read", json!({"pid": child.id(), "max_nodes": 1}), granted()).is_ok() {
                break;
            }
        }
        child
    }

    #[cfg(windows)]
    #[test]
    fn windows_never_reads_a_password_box() {
        let e = Engine::new(crate::testing::session());
        let mut child = test_window(&e);
        let tree = e.call("runtime_ui_read", json!({"pid": child.id()}), granted()).map(|o| o.to_json().to_string());
        let _ = child.kill();
        let _ = child.wait();
        let tree = tree.expect("the test window was read");
        assert!(tree.contains(r#""role":"Edit""#), "Win32 and WinForms controls are recognised: {tree}");
        assert!(!tree.contains("hunter2"), "a password box's text never shows: {tree}");
    }

    #[cfg(windows)]
    #[test]
    fn windows_runs_ui_steps_through_ui_automation() {
        let e = Engine::new(crate::testing::session());
        let mut child = test_window(&e);
        let steps = json!([
            {"action": "AXPress", "element": "hiButton"},
            {"action": "toggle", "element": "Remember me"},
            {"action": "press", "element": "No such control"},
            {"action": "wiggle", "element": "box"}
        ]);
        let r = e.call("runtime_ui_walkthrough_record", json!({"pid": child.id(), "steps": steps}), granted()).map(|o| o.to_json());
        let value = |nodes: &serde_json::Value| nodes.as_array().unwrap().iter().find(|n| n["identifier"] == "box").map(|n| n["value"].clone());
        // The click handler runs in the window's own process: on a busy machine it can land after
        // the walkthrough's last picture, so the box is read until it shows the text (or 10 s pass).
        let mut after = serde_json::Value::Null;
        for _ in 0..40 {
            after = e.call("runtime_ui_read", json!({"pid": child.id()}), granted()).map(|o| o.to_json()["result"]["nodes"].clone()).unwrap_or_default();
            if value(&after) == Some(json!("hi there")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        let _ = child.kill();
        let _ = child.wait();
        let r = r.expect("the walkthrough ran");
        let outcomes: Vec<&str> = r["result"]["steps"].as_array().unwrap().iter().map(|s| s["outcome"].as_str().unwrap()).collect();
        assert_eq!(outcomes, ["dispatched", "dispatched", "element_not_found", "unknown_action"], "{r}");
        assert_eq!(value(&r["result"]["nodes_before"]), Some(serde_json::Value::Null), "empty before");
        assert_eq!(value(&after), Some(json!("hi there")), "the press ran the button's own code");
        assert!(!r.to_string().contains("hunter2"));
        assert_eq!(r["summary"], format!("Ran 2 of 4 UI step(s) against pid {}.", child.id()));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn calls_need_consent() {
        let e = Engine::new(crate::testing::session());
        let err = e.call("runtime_ui_read", json!({"pid": 1}), CallOptions::new(Caller::Mcp)).unwrap_err();
        assert_eq!(err.code, craft_anything_evidence::ErrorCode::ConsentRequired);
    }

    /// #240: the helper answers a denied Accessibility permission with `{"error": …}` and exits 0,
    /// so the empty `nodes` list must not be reported as a successful read.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_denied_accessibility_permission_is_an_error_not_an_empty_tree() {
        use craft_anything_evidence::ErrorCode;
        let denied = json!({
            "error": "accessibility permission not granted",
            "reason": "accessibility_not_granted",
            "nodes": []
        });
        let err = super::macos::helper_error(&denied).expect("a denial is an error");
        assert_eq!(err.code, ErrorCode::NoPermission);
        assert_eq!(err.details.as_ref().unwrap()["reason"], "accessibility_not_granted");
        assert!(err.message.contains("was not read"), "{}", err.message);
        assert!(err.remediation.as_deref().is_some_and(|r| r.contains("Accessibility")), "{err:?}");
        // An empty tree the helper really read stays a successful result.
        assert!(super::macos::helper_error(&json!({"nodes": [], "truncated": false})).is_none());
        // Any other refusal is still an error, not an empty tree.
        let err = super::macos::helper_error(&json!({"error": "unknown command"})).expect("a refusal is an error");
        assert_eq!(err.code, ErrorCode::Internal);
    }

    /// #251: the callback had a literal backslash and `n` between its Python statements, which lldb
    /// rejected before it launched anything; and only breakpoint 1 got it.
    #[test]
    fn the_breakpoint_callback_is_one_valid_line_and_covers_every_breakpoint() {
        use super::{Breakpoint, ObserveCallsParams, lldb_script};
        let p = ObserveCallsParams {
            executable: "/tmp/probe".into(),
            arguments: Vec::new(),
            working_directory: None,
            breakpoints: vec![
                Breakpoint::Function { name: "alpha".into(), module: None },
                Breakpoint::ObjcMethod { class_name: "NSApplication".into(), selector: "run".into() },
            ],
            duration_ms: 10_000,
            max_events: 100,
        };
        let script = lldb_script(&p);
        let callback = script.lines().find(|l| l.starts_with("breakpoint command add")).unwrap();
        assert!(!callback.contains("\\n"), "no literal backslash-n: {callback}");
        assert!(callback.contains("print(\"CRAFT_HIT\", frame.GetFunctionName()); return False' 1 2 3"), "alpha plus both Objective-C forms: {callback}");
        // The command comes after every breakpoint it names and before the launch.
        let at = |needle: &str| script.find(needle).unwrap();
        assert!(
            at("breakpoint set --name \"+[NSApplication run]\"") < at("breakpoint command add") && at("breakpoint command add") < at("process launch"),
            "{script}"
        );
    }

    #[test]
    fn a_failed_debugger_run_is_an_error_not_zero_calls() {
        use super::lldb_failure;
        assert!(lldb_failure(true, Some(0), "").is_none());
        let e = lldb_failure(false, Some(1), "SyntaxError('unexpected character after line continuation character')").unwrap();
        assert_eq!(e.code, craft_anything_evidence::ErrorCode::ProgramFailed);
        assert!(e.message.contains("exited with code 1") && e.message.contains("SyntaxError") && e.message.contains("nothing was observed"), "{}", e.message);
        assert!(lldb_failure(false, None, "").unwrap().message.contains("stopped by the system"));
    }

    /// A line break in a name or argument would add a command to the command file.
    #[test]
    fn names_and_arguments_cannot_add_debugger_commands() {
        use super::{Breakpoint, ObserveCallsParams, single_line};
        let with = |name: &str, arg: &str| ObserveCallsParams {
            executable: "/tmp/probe".into(),
            arguments: vec![arg.into()],
            working_directory: None,
            breakpoints: vec![Breakpoint::Function { name: name.into(), module: None }],
            duration_ms: 1000,
            max_events: 1,
        };
        assert!(single_line(&with("alpha", "--flag")).is_ok());
        assert!(single_line(&with("alpha\nscript import os", "x")).is_err());
        assert!(single_line(&with("alpha", "x\r\nquit")).is_err());
    }

    #[test]
    fn module_filtered_function_breakpoints_omit_unsupported_name_mask() {
        use super::{Breakpoint, ObserveCallsParams, lldb_script};
        let p = ObserveCallsParams {
            executable: "/tmp/probe".into(),
            arguments: vec!["--flag".into()],
            working_directory: None,
            breakpoints: vec![
                Breakpoint::Function { name: "alpha".into(), module: Some("probe".into()) },
                Breakpoint::Function { name: "beta".into(), module: None },
                Breakpoint::ObjcMethod { class_name: "NSApplication".into(), selector: "run".into() },
            ],
            duration_ms: 10_000,
            max_events: 100,
        };
        let script = lldb_script(&p);
        assert!(script.contains("breakpoint set --name \"alpha\" --shlib \"probe\"\n"), "{script}");
        assert!(!script.contains("--name-mask"), "{script}");
        assert!(script.contains("breakpoint set --name \"beta\"\n"), "{script}");
    }
}
