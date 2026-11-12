//! `rebuild --loop` runs the project's saved loop through the CLI: the loop starts, and the token
//! budget saved with `project-loop-set` ends it on that limit. The stand-in builder is a `claude` on
//! PATH that answers like Claude Code's stream and reports the tokens its turn used; it then lingers,
//! as Claude Code can, so the CLI sees the usage and stops the run before the run ends on its own.
//! Each test gives the binary its own config, home and `PATH`, so the person's settings and the agents
//! installed on the machine don't decide the result.
#![cfg(unix)]

use craft_anything_engine::workspace::{TargetRef, Workspace};
use craft_anything_platform::process::{Output, Process};
use serde_json::json;
use std::path::Path;
use std::time::Duration;

/// Run the built CLI with only what the test gives it, and `stdin` as its input (empty: closed).
fn cli(args: &[&str], stdin: &[u8], config: &Path, home: &Path, bin: &Path) -> Output {
    let path = format!("{}:/usr/bin:/bin", bin.display());
    Process::new(env!("CARGO_BIN_EXE_craft-anything-cli"))
        .args(args)
        .env_clear()
        .env("CRAFT_ANYTHING_CONFIG_DIR", config)
        .env("HOME", home)
        .env("PATH", path)
        .stdin_bytes(stdin.to_vec())
        .timeout(Duration::from_secs(120))
        .run()
        .unwrap()
}

/// A stand-in `claude`: it answers `--version` and `--help` as Claude Code does. Each turn it reads
/// its prompt, reports 150,000 input tokens in its result, then lingers before it exits.
fn fake_builder(bin: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let program = bin.join("claude");
    let script = r#"#!/bin/sh
if [ "$1" = "--version" ]; then echo 'fake-claude 1.0'; exit 0; fi
if [ "$1" = "--help" ]; then echo '--output-format stream-json'; exit 0; fi
cat >/dev/null
printf '%s\n' '{"type":"system","subtype":"init","session_id":"loop-1"}'
printf '%s\n' '{"type":"assistant","message":{"content":[{"type":"text","text":"Built it."}]}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"num_turns":1,"session_id":"loop-1","usage":{"input_tokens":150000,"output_tokens":0}}'
sleep 30
"#;
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
}

/// `rebuild --loop` starts the project's saved loop, and the token budget saved with `project-loop-set`
/// ends it on that limit. A loop ended by its limit ends well, so the command succeeds.
#[test]
fn rebuild_loop_starts_and_ends_on_its_saved_token_budget() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    fake_builder(bin.path());
    let ws = Workspace::create(projects.path(), "Notes", "A notes app", TargetRef::None).unwrap();
    let root = ws.root.display().to_string();

    // Save the loop: on, one round per run, and a budget of 100k tokens (the first turn uses 150k).
    let settings = json!({"project": root, "on": true, "rounds_per_run": 1, "tokens": {"on": true, "value": 100000}}).to_string();
    let saved = cli(&["project-loop-set", "--json", "-"], settings.as_bytes(), config.path(), home.path(), bin.path());
    assert!(saved.success, "{}", saved.stderr_text());

    // Read the saved settings back with the same tool, without changing them.
    let read = cli(&["project-loop-set", "--project", root.as_str()], b"", config.path(), home.path(), bin.path());
    assert!(read.success, "{}", read.stderr_text());
    assert!(read.stdout_text().contains("100000"), "the saved budget reads back: {}", read.stdout_text());

    let run = cli(&["rebuild", "-", "Build it", "--continue-in", root.as_str(), "--loop", "--unattended"], b"", config.path(), home.path(), bin.path());
    let log = format!("{}{}", run.stdout_text(), run.stderr_text());
    assert!(log.contains("Build loop: Run 1"), "the loop starts with its first run:\n{log}");
    assert!(log.contains("The loop ended: its runs used 100k tokens, its budget."), "the budget ends the loop:\n{log}");
    assert!(run.success, "a loop ended by its limit ends well:\n{log}");
}
