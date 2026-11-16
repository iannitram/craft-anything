//! DeepSeek Harness (`dsh --profile headless`, with the task on stdin).
//!
//! The harness answers one task, streams its reasoning to stderr and prints its last message on
//! stdout, so its output is the answer text, not JSON lines.
//! This needs a headless profile that accepts piped input (verified with 0.2.2-rc.1); older
//! versions without it reject the missing task and must be updated.
//!
//! It has no flag for a tool server the way other agents do: servers come from the profile's patch
//! layers. Our tool server therefore arrives as one generated `--patch` overlay for this run, which
//! the launcher applies after the profile's own layer, so the person's saved profile is untouched.
//! The overlay is written into a folder of the run's own, owner-only on Unix, so a shared temporary
//! folder can't be used to reach it.
//! Its headless profile keeps no session to continue, so a build that stops starts again from the
//! brief (as it does with the plain adapter).

use super::*;

use std::sync::atomic::{AtomicU64, Ordering};

/// The tool server's id inside the overlay. Distinct from the one a person may have added to their
/// own profile by hand, so the two never replace each other.
const OVERLAY_ID: &str = "mcp-craft-anything-run";

/// The run's server name: not `craft-anything`, which the person's own registration uses, as the
/// harness refuses to start two clients with one name.
const OVERLAY_SERVER: &str = "craft-anything-run";

/// DeepSeek Harness run as a coding agent: one task in, its last message out.
pub struct DeepSeekHarness;

impl AgentAdapter for DeepSeekHarness {
    fn ids(&self) -> &'static [&'static str] {
        &["dsh", "deepseek-harness"]
    }

    /// Keep task text out of Windows batch arguments and their command-line length limit.
    fn prompt_on_stdin(&self) -> bool {
        true
    }

    /// The headless task includes the preamble, with no added or removed whitespace.
    fn stdin_text(&self, preamble: &str, prompt: &str) -> String {
        format!("{preamble}{prompt}")
    }

    fn args(&self, cx: &ArgsCx) -> Vec<String> {
        let mut v = cx.options.args.clone();
        v.extend(["--profile".into(), "headless".into()]);
        // A question about a build may only read the project's files, so it gets no tool server.
        if !cx.question {
            v.extend(["--patch".into(), tool_server_patch(cx.mcp)]);
        }
        // No task positional means stdin. Unlike `-`, this fails as a missing task on older
        // Harness versions without stdin support, rather than submitting a literal dash.
        v
    }

    fn streams_json(&self) -> bool {
        false
    }

    /// `--help` lists `--profile`, which the plain `dsh <task>` form would need anyway.
    fn required_flags(&self) -> (&'static [&'static str], &'static [&'static str]) {
        (&["--help"], &["--profile"])
    }

    /// Without `--profile headless` the harness boots a whole application instead of answering, so a
    /// failed flag check keeps this adapter rather than falling back to the plain one.
    fn plain_fallback(&self) -> bool {
        false
    }

    fn check_tool_server(&self, stop: &CancelToken) -> Result<(), ToolError> {
        cli_can_start(craft_anything_platform::self_cli_path().as_deref(), stop)?;
        // The folder and the write a run uses, once, before the build starts: a temporary folder
        // that can't take the overlay stops the build here with a fix, not part-way through one.
        let dir = overlay_dir()?;
        let written = craft_anything_platform::write_atomic_private(&dir.join(PROBE_FILE), b"[]\n");
        let _ = std::fs::remove_dir_all(&dir);
        written
    }
}

/// The overlay's file name inside a run's folder.
const OVERLAY_FILE: &str = "tool-server.yml";

/// The file a preflight writes and removes to prove the folder takes one.
const PROBE_FILE: &str = "probe.yml";

/// A folder of this process's own for one run, in the system temporary folder: two builds at once
/// never share one, and because it's owner-only nothing another user could prepare in a shared
/// temporary folder is ever written through.
fn overlay_dir() -> Result<std::path::PathBuf, ToolError> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("craft-anything-dsh-{}-{n}", std::process::id()));
    // `create_dir` refuses a path that is already there, so a folder prepared beforehand is never
    // used (`create_dir_all` would accept one).
    private_dir().create(&dir).map_err(|e| {
        ToolError::io(dir.display(), e).with_remediation(
            "Free space on the drive that holds the temporary folder, or point TMP at one that can be written, then start the build again.".to_string(),
        )
    })?;
    Ok(dir)
}

/// A builder for one run's own folder, owner-only from the moment it exists.
#[cfg(unix)]
fn private_dir() -> std::fs::DirBuilder {
    use std::os::unix::fs::DirBuilderExt as _;
    let mut builder = std::fs::DirBuilder::new();
    // Made owner-only as it's created, so it's never readable by anyone else, not even briefly.
    builder.mode(0o700);
    builder
}

/// A builder for one run's own folder; Windows keeps each person's temporary folder to themselves.
#[cfg(not(unix))]
fn private_dir() -> std::fs::DirBuilder {
    std::fs::DirBuilder::new()
}

/// Our tool server as a patch overlay for this run. A folder that can't take it leaves the harness
/// a path that isn't there, so the run fails at startup instead of building without the tool server
/// it was promised; [`check_tool_server`](AgentAdapter::check_tool_server) reports the everyday
/// causes first, with a fix.
fn tool_server_patch(mcp: &McpCommand) -> String {
    let Ok(dir) = overlay_dir() else { return missing_overlay() };
    let path = dir.join(OVERLAY_FILE);
    match craft_anything_platform::write_atomic_private(&path, overlay_yaml(mcp).as_bytes()) {
        Ok(()) => path.display().to_string(),
        Err(e) => {
            log::warn!("the DeepSeek Harness tool-server overlay couldn't be written: {}", e.message);
            missing_overlay()
        }
    }
}

/// A path no run wrote anything to.
fn missing_overlay() -> String {
    std::env::temp_dir().join(format!("craft-anything-dsh-{}-missing.yml", std::process::id())).display().to_string()
}

/// The overlay in the harness's patch format: a list of loader entries, one server inserted.
fn overlay_yaml(mcp: &McpCommand) -> String {
    let q = |s: &str| format!("'{}'", s.replace('\'', "''"));
    let args: String = mcp.args.iter().map(|a| format!("\n          - {}", q(a))).collect();
    format!(
        "# Craft Anything's tool server for this run. Generated; safe to delete.\n\
         - insert:\n    \
           - id: {OVERLAY_ID}\n      \
             name: '@deepseek-ai/dsh-mcp-client'\n      \
             config:\n        \
               serverName: {OVERLAY_SERVER}\n        \
               transport: stdio\n        \
               command: {}\n        \
               args:{args}\n        \
               toolCallTimeoutMs: 300000\n",
        q(&mcp.command)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tool server command for a run's project.
    fn mcp() -> McpCommand {
        McpCommand {
            command: r"C:\Program Files\Craft Anything\craft-anything-cli.exe".into(),
            args: vec!["mcp".into(), "--project".into(), r"C:\w\p\investigation.cany".into()],
            notes: None,
        }
    }

    /// The arguments for one message, and the server they name.
    fn args(options: &BackendOptions, question: bool) -> (Vec<String>, McpCommand) {
        let mcp = mcp();
        let cx = ArgsCx {
            options,
            prompt: "make a game",
            preamble: "PRE ",
            mcp: &mcp,
            resume: None,
            builder: !question,
            reads_checked: false,
            question,
            message_file: None,
        };
        (DeepSeekHarness.args(&cx), mcp.clone())
    }

    /// Only options go in argv; the task arrives on stdin after the profile starts.
    #[test]
    fn headless_argv_contains_only_profile_and_patch_options() {
        let (a, _) = args(&BackendOptions::default(), false);
        assert_eq!(a[0..2], ["--profile", "headless"], "{a:?}");
        assert_eq!(a.len(), 4, "only the profile and tool-server patch: {a:?}");
        let with_args = BackendOptions { args: vec!["--patch".into(), "extra.yml".into()], ..Default::default() };
        let (a, _) = args(&with_args, false);
        assert_eq!(a[0..4], ["--patch", "extra.yml", "--profile", "headless"], "{a:?}");
        assert_eq!(a.len(), 6, "custom options and the generated patch: {a:?}");
    }

    /// The run's tool server, as an overlay in a folder of its own that only its owner can read.
    #[test]
    fn the_tool_server_arrives_as_a_patch_overlay_in_the_runs_own_folder() {
        let (a, mcp) = args(&BackendOptions::default(), false);
        let i = a.iter().position(|x| x == "--patch").expect("a --patch overlay");
        let path = std::path::PathBuf::from(&a[i + 1]);
        assert!(path.exists(), "{path:?}");
        let written = std::fs::read_to_string(&path).expect("read the overlay");
        assert!(written.contains("- insert:"), "{written}");
        assert!(written.contains(OVERLAY_ID), "{written}");
        assert!(written.contains("serverName: craft-anything-run\n"), "the person's own registration keeps its name: {written}");
        // The server's own arguments, including the run's project, quoted for YAML.
        assert!(written.contains("- 'mcp'"), "{written}");
        assert!(written.contains(&format!("- '{}'", mcp.args[2])), "{written}");
        // A folder of the run's own, so nothing another user prepared in a shared temporary folder
        // is written through.
        let folder = path.parent().expect("a folder of its own").to_path_buf();
        let own = format!("craft-anything-dsh-{}-", std::process::id());
        assert!(folder.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&own)), "{folder:?}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let bits = |p: &std::path::Path| std::fs::metadata(p).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(bits(&folder), 0o700, "folder {:o}", bits(&folder));
            assert_eq!(bits(&path), 0o600, "overlay {:o}", bits(&path));
        }
        // Two runs never share one.
        let (b, _) = args(&BackendOptions::default(), false);
        let other = std::path::PathBuf::from(&b[b.iter().position(|x| x == "--patch").expect("--patch") + 1]);
        let other_dir = other.parent().expect("a folder of its own").to_path_buf();
        assert_ne!(folder, other_dir);
        for dir in [folder, other_dir] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// A folder that can't take the overlay stops the run, rather than building without the tool
    /// server it was promised.
    #[test]
    fn a_folder_that_cannot_take_the_overlay_leaves_a_path_that_is_not_there() {
        let missing = missing_overlay();
        assert!(!std::path::Path::new(&missing).exists(), "{missing}");
    }

    /// A question may only read, so it gets no tool server.
    #[test]
    fn a_question_gets_no_tool_server() {
        let (a, _) = args(&BackendOptions::default(), true);
        assert!(!a.iter().any(|x| x == "--patch"), "{a:?}");
        assert_eq!(a, ["--profile", "headless"], "{a:?}");
    }

    /// Batch launchers cannot accept line breaks or long tasks in argv; stdin preserves the text.
    #[test]
    fn multiline_and_long_tasks_use_stdin_without_changing_their_bytes() {
        let preamble = "Instructions:\r\n";
        let tasks = ["-question?\r\n中文 %PATH% & ^ \"quoted\"\n\n".to_string(), format!("{}\nlast line\n", "x".repeat(40_831))];
        for id in ["dsh", "deepseek-harness"] {
            let adapter = AgentAdapters::built_in().for_agent(id);
            for task in &tasks {
                for access in [Access::Build, Access::Question] {
                    let a = build_args(adapter.as_ref(), &BackendOptions::default(), task, preamble, &mcp(), None, access);
                    assert!(adapter.prompt_on_stdin(), "{id} must send task text on stdin");
                    assert_eq!(a.len(), if matches!(access, Access::Question) { 2 } else { 4 }, "only options: {a:?}");
                    assert!(!a.iter().any(|arg| arg.contains(task) || arg.contains(preamble)), "{a:?}");
                    assert!(a.iter().map(|arg| arg.len() + 1).sum::<usize>() < 8191, "the task must not grow argv");
                    assert_eq!(adapter.stdin_text(preamble, task), format!("{preamble}{task}"), "{id}");
                }
            }
        }
    }

    /// The agent is registered under both ids, not handed to the plain adapter.
    #[test]
    fn registered_not_the_plain_adapter() {
        assert!(AgentAdapters::built_in().for_agent("dsh").ids().contains(&"dsh"));
        assert!(AgentAdapters::built_in().for_agent("deepseek-harness").ids().contains(&"dsh"));
        assert_eq!(DeepSeekHarness.required_flags(), (&["--help"][..], &["--profile"][..]));
        assert!(!DeepSeekHarness.plain_fallback());
        assert!(!DeepSeekHarness.streams_json());
    }

    /// A path or a flag with a quote in it must not break the overlay's YAML.
    #[test]
    fn quotes_are_escaped_for_yaml() {
        let mcp = McpCommand { command: "cli's.exe".into(), args: vec!["--project".into(), r"C:\it's here".into()], notes: None };
        let y = overlay_yaml(&mcp);
        assert!(y.contains("command: 'cli''s.exe'"), "{y}");
        assert!(y.contains("- 'C:\\it''s here'"), "{y}");
    }
}
