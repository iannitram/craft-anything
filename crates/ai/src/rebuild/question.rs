//! Questions about a build ("Just ask"): "where are the latest files?", "why did it pick
//! Bevy?". They're answered without starting a build or a round, and nothing is changed:
//!
//! - a coding agent that keeps a session (Claude Code) answers from a copy of the builder's own
//!   session (`--fork-session`), which isn't kept, with only Read, Glob and Grep (none in Clean
//!   room) and no tool server, so the builder's session and files stay as they are;
//! - Copilot starts a fresh ACP session with no tool server: only view, glob and grep in Deep
//!   mode, no tools in Clean room. It answers from the project context, not the builder's chat;
//! - an AI with an API key answers with our agent loop on a [`ReadOnlyHost`]: the engine's tools
//!   that change nothing, pointed at this project in an engine of its own;
//! - any other agent is refused, since it can't be held to reading.
//!
//! Every question carries what the project is and its most recently changed files, so a plain
//! "where is…" is answered even without tools.

use super::*;
use crate::agent::{EngineHost, LoopConfig, ReadOnlyHost};
use crate::cli_agents::CliSpec;
use craft_anything_engine::Toolset;
use serde::Serialize;
use std::time::{Duration, Instant, SystemTime};

/// Longest a question may take.
pub const WAIT: Duration = Duration::from_secs(10 * 60);
/// Longest question.
const MAX_QUESTION: usize = 4000;
/// How many recently changed files the question carries.
const RECENT_FILES: usize = 25;
/// How many files are looked at to find them, and how deep.
const SCAN_FILES: usize = 20_000;
const SCAN_DEPTH: usize = 12;
/// Folders a build makes again, never listed.
const SKIPPED: &[&str] = &["node_modules", "target", ".godot", "Library", "Temp", "obj", ".venv", "__pycache__", ".next", ".turbo", ".git"];

const PREAMBLE: &str = "You're answering a question the person asked about the project you built inside Craft Anything. This isn't a task: answer it and stop. Don't create, change or delete any file, don't run anything, and don't carry on the build; if the answer is that something should change, say what you'd change and that they can ask for it as a change. Answer in plain words for someone who doesn't code, briefly, with full paths when they ask where something is. You're a short-lived copy with fewer tools than the builder, and the build may still be running beside you. So a tool you can't see here is not a tool the build has lost, and a call with no result here may still be running: never report tools as disconnected or work as cut off because of what you see from here.\n\n";

/// How a question was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnsweredFrom {
    /// A copy of the builder's own session (it remembers the build).
    BuildersSession,
    /// A fresh conversation, from the project's files and what the question carries.
    TheFiles,
}

/// A question's answer.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Answered {
    pub answer: String,
    pub from: AnsweredFrom,
    /// The AI that answered.
    pub backend: String,
    pub cost_usd: Option<f64>,
    /// The cost is the agent's own estimate at API prices, not a key's charge.
    #[serde(default)]
    pub cost_is_estimate: bool,
    /// The saved builder that was off or gone, when another AI answered in its place.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub builder_notice: Option<String>,
}

/// Where a question's answer comes from: the builder's session only when the project has one for
/// this mode (the caller filters it) and the adapter can continue it; otherwise the files.
fn answered_from(session: Option<&str>, resumes: bool) -> AnsweredFrom {
    if session.is_some() && resumes { AnsweredFrom::BuildersSession } else { AnsweredFrom::TheFiles }
}

/// Answer `question` about `ws` (see the module docs), with the person's `settings` (taken from
/// the caller, which may hold the session: a tool does while it runs, so locking it here would
/// wait forever). Blocks until it's answered, `cancel` is set, or [`WAIT`] passes: call it off
/// the UI thread.
pub fn ask(
    hub: &AiHub,
    settings: &craft_anything_engine::Settings,
    logs: Option<std::path::PathBuf>,
    ws: &Workspace,
    question: &str,
    cancel: &CancelToken,
) -> Result<Answered, ToolError> {
    let question = question.trim();
    if question.is_empty() || question.len() > MAX_QUESTION {
        return Err(ToolError::invalid(format!("A question is 1 to {MAX_QUESTION} characters.")));
    }
    let cfg = crate::AiConfig::from_settings(settings);
    let builder_notice = super::builder_replacement(&cfg, ws, None).map(|r| super::replaced_builder_notice(&r.saved, &r.used));
    let entry = builder_backend(&cfg, project_builder(&cfg, ws))?;
    let deep = settings.deep_mode;
    super::refuse_deep_session_reuse_in_clean_room(ws, &entry.config.model, &entry.options.args, deep)?;
    let cli = |resume: Option<String>| -> Box<dyn Conversation> {
        let spec = CliSpec {
            question: true,
            deep,
            logs: logs.clone(),
            resume,
            key: crate::cli_agents::acp::key_for(&hub.keys, &entry.config.model, &entry.config.name, entry.config.kind),
            preamble: Some(PREAMBLE.into()),
            ..CliSpec::new(entry.config.clone(), entry.options.clone(), false, Some(ws.root.clone()))
        };
        crate::cli_agents::conversation(spec)
    };
    let (mut conv, mut from, builder_session): (Box<dyn Conversation>, AnsweredFrom, Option<String>) = match entry.config.kind {
        BackendKind::AgentCli => {
            let session = ws.manifest.agent_session_for_mode(&entry.config.model, deep).map(str::to_string);
            if let Some(id) = session.as_deref() {
                bring_back_session(ws, &entry, id);
            }
            // Only a native adapter that resumes can answer from a copy of the builder's chat.
            // Copilot's ACP question starts fresh, even when the project has a saved session id.
            // An adapter that resumes (Codex) but can't hold a question to reading has no copy to fork.
            let adapter = crate::cli_agents::adapter_for(&entry.config.model);
            let native_session = adapter.resumes() && adapter.answers_questions();
            let from = answered_from(session.as_deref(), native_session);
            let builder_session = session.clone();
            let spec = CliSpec {
                question: true,
                deep,
                logs: logs.clone(),
                resume: session,
                preamble: Some(PREAMBLE.into()),
                ..CliSpec::new(entry.config.clone(), entry.options.clone(), false, Some(ws.root.clone()))
            };
            (crate::cli_agents::conversation(spec), from, builder_session)
        }
        _ => {
            // An engine of its own, pointed at this project, so a build running meanwhile keeps its.
            let engine = Arc::new(Engine::new(craft_anything_engine::Session::new(settings.clone())));
            engine.session().ext.insert(Arc::new(InboxDir(ws.inbox_dir())));
            let host = Arc::new(ReadOnlyHost(EngineHost(engine)));
            let slot = hub.build_model(&entry)?;
            let toolsets = [Toolset::Core, Toolset::Build].into_iter().collect();
            let conv =
                crate::agent::ApiConversation::start_with(host, vec![slot], LoopConfig { prices: cfg.prices.clone(), ..LoopConfig::default() }, toolsets)?;
            (Box::new(conv), AnsweredFrom::TheFiles, None)
        }
    };
    let prompt = |from| match from {
        AnsweredFrom::BuildersSession => format!("{}\n\nQuestion: {question}", context(ws)),
        AnsweredFrom::TheFiles => format!("{PREAMBLE}{}\n\nQuestion: {question}", context(ws)),
    };
    // Read the builder's total before the question starts: a build turn that finishes meanwhile
    // may advance the same session, and the fork's total includes that too.
    let cost_file = logs.as_deref().and_then(Path::parent).map(crate::cli_agents::claude_cost::path_in);
    let mut builder_before = if from == AnsweredFrom::BuildersSession && entry.config.model == "claude-code" {
        cost_file.as_deref().zip(builder_session.as_deref()).and_then(|(path, builder)| crate::cli_agents::claude_cost::kept_total(path, builder))
    } else {
        None
    };
    conv.send(&prompt(from));
    let mut answered = collect(conv.as_mut(), cancel, WAIT);
    // The builder's session isn't on this computer (the project was copied here): the question
    // is answered from the files instead.
    if answered.is_err() && from == AnsweredFrom::BuildersSession && conv.session_lost() {
        // Forgotten, so later questions don't ask for it again. Not saving it isn't worth failing
        // the question over: the next build finds it missing too, forgets it and says so.
        if let Some(lost) = ws.manifest.agent_session.as_deref() {
            let _ = forget_lost_session(&ws.root, lost);
        }
        from = AnsweredFrom::TheFiles;
        builder_before = None;
        conv = cli(None);
        conv.send(&prompt(from));
        answered = collect(conv.as_mut(), cancel, WAIT);
    }
    let (answer, cost_usd, cost_is_estimate) = answered?;
    // A copy of a session reports the whole session's running cost, not this question's (a
    // live run said $22.67 for a few cents of cache reads): the difference from the builder's
    // kept total is what the question added.
    let (cost_usd, cost_is_estimate) = if from == AnsweredFrom::BuildersSession {
        // Claude Code reports the whole session's total; other adapters' costs stay as reported.
        if entry.config.model == "claude-code" {
            let cost = cost_usd.and_then(|total| crate::cli_agents::claude_cost::fork_cost_from(builder_before, Some(total)));
            (cost, cost.is_some())
        } else {
            // A fork of another agent's session is still the whole session's total; don't show it.
            (None, false)
        }
    } else {
        (cost_usd, cost_is_estimate)
    };
    Ok(Answered { answer, from, backend: entry.config.name.clone(), cost_usd, cost_is_estimate, builder_notice })
}

/// The builder's session, kept in the project, put back where its agent looks when this computer
/// doesn't have it (#451), so the question is answered from the builder's own conversation. Only
/// for an agent here (a remote one keeps its sessions there). Without it the question is answered
/// from the files, as when nothing was kept, so a failure here only costs that.
fn bring_back_session(ws: &Workspace, entry: &crate::BackendEntry, id: &str) {
    if entry.config.ssh_host.as_deref().is_some_and(|h| !h.trim().is_empty()) {
        return;
    }
    if let Some(saved) = super::kept_session_bytes(ws) {
        let adapter = crate::cli_agents::AgentAdapters::default().for_backend(&entry.config.model, &entry.options);
        let _ = adapter.restore_session(id, &saved, &ws.root);
    }
}

/// Forget the project's builder session `lost`, which this computer doesn't have, as the build
/// does. A question may be asked while a build runs, so this reads the project again and only
/// clears the id if it's still `lost`: a build may have saved another since. Returns whether it
/// was cleared.
fn forget_lost_session(root: &Path, lost: &str) -> Result<bool, ToolError> {
    let mut now = Workspace::open(root)?;
    if now.manifest.agent_session.as_deref() != Some(lost) {
        return Ok(false);
    }
    now.manifest.agent_session = None;
    now.manifest.agent_session_deep = None;
    now.save()?;
    Ok(true)
}

/// The answer's text and cost, once its turn is done. `cost_is_estimate` says whether the cost is
/// the agent's own estimate rather than a key's charge.
fn collect(conv: &mut dyn Conversation, cancel: &CancelToken, wait: Duration) -> Result<(String, Option<f64>, bool), ToolError> {
    let start = Instant::now();
    let mut text = String::new();
    let mut idle_since: Option<Instant> = None;
    loop {
        for e in conv.poll() {
            match e {
                AgentEvent::TextDelta { text: t } => text.push_str(&t),
                AgentEvent::TurnDone { cost_usd, cost_is_estimate, .. } => return Ok((text.trim().to_string(), cost_usd, cost_is_estimate)),
                AgentEvent::Error { message, remediation, .. } => {
                    let e = ToolError::new(ErrorCode::ProgramFailed, message);
                    return Err(match remediation {
                        Some(r) => e.with_remediation(r),
                        None => e,
                    });
                }
                _ => {}
            }
        }
        if cancel.is_cancelled() {
            conv.stop();
            return Err(ToolError::new(ErrorCode::Cancelled, "The question was stopped."));
        }
        if start.elapsed() > wait {
            conv.stop();
            return Err(ToolError::new(ErrorCode::TimedOut, "No answer came in time.").with_remediation("Ask again, or ask something shorter."));
        }
        // Finished without saying it was done: whatever it said is the answer.
        if conv.is_busy() {
            idle_since = None;
        } else if idle_since.get_or_insert_with(Instant::now).elapsed() > Duration::from_secs(2) {
            if text.trim().is_empty() {
                return Err(ToolError::new(ErrorCode::ProgramFailed, "The AI stopped without answering.").with_remediation("Ask again."));
            }
            return Ok((text.trim().to_string(), None, false));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// What the project is and where its files are, for any question.
fn context(ws: &Workspace) -> String {
    let m = &ws.manifest;
    let mut s = format!(
        "Project: {}\nWhat the person asked for: {}\nProject folder: {}\nThe app's folder: {}\nStatus: {}",
        m.name,
        m.request,
        ws.root.display(),
        ws.app_dir().display(),
        m.status.label()
    );
    if !m.stack.is_empty() {
        s.push_str(&format!("\nBuilt with: {}", m.stack));
    }
    let recent = recent_files(&ws.app_dir());
    if !recent.is_empty() {
        s.push_str("\nMost recently changed files in the app (newest first):");
        for (p, ago) in recent {
            s.push_str(&format!("\n- {} ({ago})", p.display()));
        }
    }
    s
}

/// The most recently changed files under `app`, newest first, with how long ago; what a build
/// makes again (dependencies, compiler output) is left out.
pub fn recent_files(app: &Path) -> Vec<(PathBuf, String)> {
    let mut found: Vec<(SystemTime, PathBuf)> = Vec::new();
    let mut stack = vec![(app.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            if found.len() >= SCAN_FILES {
                break;
            }
            let Ok(kind) = e.file_type() else { continue };
            let name = e.file_name();
            if kind.is_dir() {
                if depth < SCAN_DEPTH && !name.to_str().is_some_and(|n| SKIPPED.contains(&n)) {
                    stack.push((e.path(), depth + 1));
                }
            } else if kind.is_file()
                && let Ok(at) = e.metadata().and_then(|m| m.modified())
            {
                found.push((at, e.path()));
            }
        }
    }
    found.sort_by_key(|f| std::cmp::Reverse(f.0));
    let now = SystemTime::now();
    found.into_iter().take(RECENT_FILES).map(|(at, p)| (p, ago(now.duration_since(at).unwrap_or_default()))).collect()
}

fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", s / 60),
        3600..86_400 => format!("{} h ago", s / 3600),
        86_400..172_800 => "yesterday".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file changed between 24 and 48 hours ago was listed as "1 days ago".
    #[test]
    fn recent_file_age_says_yesterday_for_the_second_day() {
        assert_eq!(ago(Duration::from_secs(86_399)), "23 h ago");
        assert_eq!(ago(Duration::from_secs(86_400)), "yesterday");
        assert_eq!(ago(Duration::from_secs(86_405)), "yesterday");
        assert_eq!(ago(Duration::from_secs(172_799)), "yesterday");
        assert_eq!(ago(Duration::from_secs(172_800)), "2 days ago");
    }

    /// A copy of the builder's session has no tool server, and saw the build's tools as gone.
    #[test]
    fn a_question_is_told_its_missing_tools_are_not_the_builds() {
        assert!(PREAMBLE.contains("never report tools as disconnected"));
        assert!(PREAMBLE.contains("may still be running"));
    }

    /// The user-visible half of R2.9: a Deep session isn't used for a Clean room question, and an
    /// adapter that can't resume answers from the files even when the mode matches.
    #[test]
    fn a_question_answers_from_the_session_only_in_the_mode_it_was_saved_in() {
        let d = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(d.path(), "Notes", "A notes app", TargetRef::None).unwrap();
        ws.manifest.agent_session = Some("s1".into());
        ws.manifest.agent_session_by = Some("claude-code".into());
        ws.manifest.agent_session_deep = Some(true);
        let deep = ws.manifest.agent_session_for_mode("claude-code", true);
        assert_eq!(answered_from(deep, true), AnsweredFrom::BuildersSession);
        let clean = ws.manifest.agent_session_for_mode("claude-code", false);
        assert_eq!(answered_from(clean, true), AnsweredFrom::TheFiles, "a Deep session isn't used in Clean room");
        assert_eq!(answered_from(deep, false), AnsweredFrom::TheFiles, "an adapter that can't resume answers from the files");
    }

    /// R2.9 through `ask`: a Clean room question passes no resume to the agent and reports the
    /// files; a Deep question with a Deep session resumes it. A saved `--resume`/`--continue` is
    /// refused before the agent starts.
    #[cfg(unix)]
    #[test]
    fn a_question_uses_the_session_only_in_the_mode_it_was_saved_in() {
        use crate::{AiHub, BackendConfig, BackendEntry, BackendKind, BackendOptions, KeyStore};
        use std::os::unix::fs::PermissionsExt;

        fn ask_with(deep: bool, saved_args: bool, baseline: Option<f64>, advance: Option<f64>) -> (Result<(Answered, String), ToolError>, bool) {
            let d = tempfile::tempdir().unwrap();
            let program = d.path().join("claude");
            let mut script = String::from("#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" > \"$d/argv\"\n");
            if let Some(advance) = advance {
                script.push_str(&format!("printf '{{\"version\":1,\"totals\":{{\"deep-session\":{advance}}}}}' > \"$d/claude-session-costs.json\"\n"));
            }
            script.push_str("cat \"$d/reply.jsonl\"\n");
            std::fs::write(&program, script).unwrap();
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::write(
                d.path().join("reply.jsonl"),
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"It is a notes app.\"}]}}\n{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"session_id\":\"new-session\",\"total_cost_usd\":1.02}\n",
            )
            .unwrap();
            if let Some(baseline) = baseline {
                std::fs::write(d.path().join("claude-session-costs.json"), format!("{{\"version\":1,\"totals\":{{\"deep-session\":{baseline}}}}}")).unwrap();
            }
            let engine = Arc::new(Engine::new(craft_anything_engine::testing::session()));
            let hub = AiHub::with_stores(engine, d.path().join("settings.toml"), KeyStore::file_only(d.path().join("keys.toml")));
            let mut cfg = hub.config();
            cfg.upsert(BackendEntry {
                config: BackendConfig {
                    name: "Claude".into(),
                    kind: BackendKind::AgentCli,
                    model: "claude-code".into(),
                    base_url: None,
                    ssh_host: None,
                    enabled: true,
                },
                options: BackendOptions {
                    command: Some(program.display().to_string()),
                    args: if saved_args { vec!["--resume".into(), "deep-session".into()] } else { Vec::new() },
                    ..Default::default()
                },
            });
            hub.engine.session().settings.ai = cfg.to_table().unwrap();
            let mut settings = hub.engine.session().settings.clone();
            settings.deep_mode = deep;
            let mut ws = Workspace::create(d.path(), "Notes", "A notes app", TargetRef::None).unwrap();
            ws.manifest.agent_session = Some("deep-session".into());
            ws.manifest.agent_session_by = Some("claude-code".into());
            ws.manifest.agent_session_deep = Some(true);
            ws.save().unwrap();
            let answered = ask(&hub, &settings, Some(d.path().join("logs")), &ws, "What is it?", &CancelToken::default());
            let argv = std::fs::read_to_string(d.path().join("argv")).unwrap_or_default();
            let spawned = !argv.is_empty();
            (answered.map(|a| (a, argv)), spawned)
        }

        let (clean, clean_spawned) = ask_with(false, false, Some(1.0), None);
        assert!(clean_spawned, "Clean room spawns the agent");
        let (clean, clean_argv) = clean.unwrap();
        assert_eq!(clean.from, AnsweredFrom::TheFiles);
        assert!(!clean_argv.lines().any(|a| a == "--resume" || a == "deep-session"), "{clean_argv:?}");

        let (deep, deep_spawned) = ask_with(true, false, Some(1.0), None);
        assert!(deep_spawned, "Deep spawns the agent");
        let (deep, deep_argv) = deep.unwrap();
        assert_eq!(deep.from, AnsweredFrom::BuildersSession);
        assert!(deep_argv.lines().any(|a| a == "--resume") && deep_argv.lines().any(|a| a == "deep-session"), "{deep_argv:?}");
        let cost = deep.cost_usd.unwrap();
        assert!((cost - 0.02).abs() < 1e-9, "the fork's total less the builder's kept total: {cost}");
        assert!(deep.cost_is_estimate);

        let (unknown, unknown_spawned) = ask_with(true, false, None, None);
        assert!(unknown_spawned);
        let (unknown, _) = unknown.unwrap();
        assert_eq!(unknown.cost_usd, None, "without a kept builder total the whole session's total isn't this question's");

        let (advanced, advanced_spawned) = ask_with(true, false, Some(1.0), Some(1.01));
        assert!(advanced_spawned);
        let (advanced, _) = advanced.unwrap();
        let cost = advanced.cost_usd.unwrap();
        assert!((cost - 0.02).abs() < 1e-9, "the builder's total is read before the question starts: {cost}");

        let (err, spawned) = ask_with(false, true, Some(1.0), None);
        assert!(!spawned, "refused before the agent starts");
        assert!(err.unwrap_err().message.contains("saved in Deep mode"), "the guard's message");
    }

    /// A question that finds the builder's session missing forgets it, unless a build has saved
    /// another meanwhile: then that one stays.
    #[test]
    fn a_lost_session_is_forgotten_but_never_one_a_build_saved_since() {
        let d = tempfile::tempdir().unwrap();
        let mut ws = Workspace::create(d.path(), "Notes", "A notes app", TargetRef::None).unwrap();
        ws.manifest.agent_session = Some("gone".into());
        ws.manifest.agent_session_deep = Some(true);
        ws.manifest.last_reply = "Built it.".into();
        ws.save().unwrap();
        assert!(forget_lost_session(&ws.root, "gone").unwrap());
        let now = Workspace::open(&ws.root).unwrap();
        assert_eq!(now.manifest.agent_session, None);
        assert_eq!(now.manifest.agent_session_deep, None, "the mode goes with the session");
        assert_eq!(now.manifest.last_reply, "Built it.", "nothing else changes");

        ws.manifest.agent_session = Some("new-from-a-build".into());
        ws.manifest.agent_session_deep = Some(false);
        ws.save().unwrap();
        assert!(!forget_lost_session(&ws.root, "gone").unwrap());
        let kept = Workspace::open(&ws.root).unwrap();
        assert_eq!(kept.manifest.agent_session.as_deref(), Some("new-from-a-build"));
        assert_eq!(kept.manifest.agent_session_deep, Some(false), "a session a build saved since keeps its mode");

        assert!(forget_lost_session(&d.path().join("no-such-project"), "gone").is_err(), "an error, never a panic");
    }

    #[test]
    fn the_newest_files_come_first_and_build_output_is_left_out() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path();
        std::fs::create_dir_all(app.join("src")).unwrap();
        std::fs::create_dir_all(app.join("target/release")).unwrap();
        std::fs::write(app.join("Cargo.toml"), "").unwrap();
        std::thread::sleep(Duration::from_millis(30));
        std::fs::write(app.join("target/release/big.exe"), "").unwrap();
        std::fs::write(app.join("src/main.rs"), "").unwrap();
        let files: Vec<PathBuf> = recent_files(app).into_iter().map(|(p, _)| p).collect();
        assert_eq!(files, vec![app.join("src/main.rs"), app.join("Cargo.toml")]);
    }

    /// A conversation that says a few words, then reports its turn done.
    struct Scripted(Vec<AgentEvent>);
    impl Conversation for Scripted {
        fn send(&mut self, _: &str) {}
        fn poll(&mut self) -> Vec<AgentEvent> {
            std::mem::take(&mut self.0)
        }
        fn answer_consent(&mut self, _: &str, _: bool) {}
        fn stop(&mut self) {}
        fn is_busy(&self) -> bool {
            true
        }
    }

    #[test]
    fn an_answer_is_its_words_until_the_turn_ends_and_an_error_is_an_error() {
        let words = |t: &str| AgentEvent::TextDelta { text: t.into() };
        let done = AgentEvent::TurnDone { backend: "A".into(), input_tokens: 1, output_tokens: 1, cost_usd: Some(0.02), cost_is_estimate: false };
        let mut ok = Scripted(vec![words("They're in "), words("app/src."), done]);
        assert_eq!(collect(&mut ok, &CancelToken::default(), WAIT).unwrap(), ("They're in app/src.".to_string(), Some(0.02), false));
        let mut bad = Scripted(vec![AgentEvent::Error { message: "not signed in".into(), remediation: Some("Sign in".into()), log: None }]);
        let e = collect(&mut bad, &CancelToken::default(), WAIT).unwrap_err();
        assert_eq!((e.message.as_str(), e.remediation.as_deref()), ("not signed in", Some("Sign in")));
        let mut slow = Scripted(Vec::new());
        assert_eq!(collect(&mut slow, &CancelToken::default(), Duration::from_millis(100)).unwrap_err().code, ErrorCode::TimedOut);
    }
}
