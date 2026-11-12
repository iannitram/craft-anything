//! `rebuild`: the whole study → build → check workflow, headless. Questions and approvals are
//! answered on the terminal, or automatically with `--unattended` (first choice; approvals are
//! refused, so nothing outside the project happens without a person).

use crate::{Cli, INTERRUPTED, make_engine};
use clap::Args;
use craft_anything_ai::rebuild::build_loop::{self as loop_rules, LoopEnd, LoopState, Next};
use craft_anything_ai::rebuild::{self, RebuildRun, RunEvent, RunOptions};
use craft_anything_ai::{AgentEvent, AiHub, BackendKind};
use craft_anything_engine::inbox::{Answer, Request, RequestKind, now_ms};
use craft_anything_engine::workspace::Workspace;
use craft_anything_engine::workspace::build_loop::LoopSettings;
use craft_anything_evidence::ToolError;
use std::io::{BufRead as _, Write as _};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Debug, Args)]
pub struct RebuildArgs {
    /// A web address, an absolute path to an app, game or file, or `-` for none. A folder that
    /// already holds a project keeps its saved target.
    pub target: String,
    /// What to make, in your words; a folder that already holds a project gets it as a follow-up.
    pub request: String,
    /// Where the new project folder goes (default: the projects folder from settings).
    #[arg(long, conflicts_with = "project_folder")]
    pub projects_dir: Option<PathBuf>,
    /// Start the new project in this folder itself (it's added to the project list). If it
    /// already has files, --existing says what they are; if it already holds a project, its saved
    /// target, settings and stack are kept and the request becomes a follow-up.
    #[arg(long, conflicts_with = "continue_in")]
    pub project_folder: Option<PathBuf>,
    /// With --project-folder that has files and isn't a project yet: `edit` (they're the app,
    /// edited in place) or `start-from` (material to start from; the app is built in app/ beside
    /// them).
    #[arg(long, requires = "project_folder", value_parser = ["edit", "start-from"])]
    pub existing: Option<String>,
    /// Continue an existing project folder; the request becomes a follow-up message.
    #[arg(long)]
    pub continue_in: Option<PathBuf>,
    /// With --continue-in: carry on where the project's last run stopped (paused, or the app or
    /// the computer stopped); the request is ignored.
    #[arg(long, requires = "continue_in")]
    pub resume: bool,
    /// The coding agent to use (an enabled backend name from `ai list`, or an installed agent
    /// such as `Claude Code`).
    #[arg(long)]
    pub backend: Option<String>,
    /// Refinement rounds after the first build.
    #[arg(long, default_value_t = 2)]
    pub rounds: u32,
    /// Answer questions with the first choice and refuse approvals, without asking.
    #[arg(long)]
    pub unattended: bool,
    /// What to build with: an engine (unity, unreal, godot, rust) or a stack's full name.
    /// New projects only; otherwise it's chosen from the request and target.
    #[arg(long)]
    pub build_with: Option<String>,
    /// Let the builder generate pictures and 3D models for a new game (needs a fal.ai key; paid
    /// within the project's asset budget). Change it later with `project-assets-allow`.
    #[arg(long)]
    pub generate_assets: bool,
    /// Keep building with the project's build loop: when a run ends well, the next starts with
    /// a fresh builder that carries on from the last one's handoff, until a limit saved with
    /// `project-loop-set` ends it (each run's rounds replace --rounds). With none saved: 2 rounds
    /// a run, at most 10 runs, ending after 2 runs in a row that change nothing.
    #[arg(long = "loop")]
    pub build_loop: bool,
}

pub fn rebuild(cli: &Cli, a: &RebuildArgs) -> u8 {
    match run(cli, a) {
        Ok(true) => 0,
        Ok(false) => 1,
        Err(e) => {
            eprintln!("rebuild: {}", e.message);
            if let Some(r) = e.remediation {
                eprintln!("  {r}");
            }
            1
        }
    }
}

/// Add the agent app this run asks for, or the first one found when no enabled AI can build.
fn ensure_agent(hub: &AiHub, requested: Option<&str>) -> Result<(), ToolError> {
    let configured = hub.backends();
    // An enabled backend can build, so a run that names none needs no agent app.
    if requested.is_none() && configured.iter().any(|b| b.enabled) {
        return Ok(());
    }
    // A name that is configured is left alone (including a backend that is off): don't add a
    // second one under the same name; the run itself refuses a backend that is off.
    if requested.is_some_and(|name| configured.iter().any(|b| b.name == name)) {
        return Ok(());
    }
    // An explicit name may be an installed agent that isn't added yet: add only that one. With no
    // name, add the first agent found.
    if let Some(d) = hub.detect().into_iter().find(|d| d.suggestion.kind == BackendKind::AgentCli && requested.is_none_or(|name| d.suggestion.name == name)) {
        eprintln!("Using {} ({}).", d.suggestion.name, d.reason);
        hub.add_backend(d.suggestion, None)?;
    }
    Ok(())
}

/// The `rebuild` command: open or make the project, start the run, and print its events until it ends.
fn run(cli: &Cli, a: &RebuildArgs) -> Result<bool, ToolError> {
    let engine = make_engine(None, cli.deep_for_run())?;
    let hub = AiHub::new(engine.clone());
    // Before the build starts, while no session lock is held (#1225).
    if let Err(e) = hub.ensure_session_id() {
        log::warn!("couldn't save the OpenCode Go session id: {e}");
    }
    if a.build_with.is_some() && a.continue_in.is_some() {
        return Err(ToolError::invalid("--build-with is for new projects; a project keeps the stack it was built with.")
            .with_remediation("Leave out --build-with, or start a new project."));
    }
    if a.generate_assets && a.continue_in.is_some() {
        return Err(ToolError::invalid("--generate-assets is for new projects.").with_remediation("Use project-assets-allow for an existing project."));
    }
    ensure_agent(&hub, a.backend.as_deref())?;
    // Refuse an unknown or switched-off name before opening or making a project: a command that
    // can't build must not leave a new folder behind.
    if let Some(name) = a.backend.as_deref() {
        rebuild::builder_backend(&hub.config(), Some(name))?;
    }
    let (ws, message) = match &a.continue_in {
        Some(dir) => (Workspace::open(&std::path::absolute(dir).map_err(|e| ToolError::io(dir.display(), e))?)?, (!a.resume).then(|| a.request.clone())),
        None => {
            use craft_anything_ai::rebuild::project::Place;
            use craft_anything_engine::workspace::ExistingFiles;
            let settings = engine.session().settings.clone();
            let existing = a.existing.as_deref().map(|e| if e == "edit" { ExistingFiles::Edit } else { ExistingFiles::StartFrom });
            let folder = a.project_folder.as_deref().map(std::path::absolute).transpose().map_err(|e| ToolError::io("--project-folder", e))?;
            let attached = folder.as_ref().is_some_and(|f| rebuild::project::has_project(f));
            if attached {
                if a.existing.is_some() {
                    return Err(ToolError::invalid("This folder already holds a project; use --continue-in to continue it."));
                }
                if a.build_with.is_some() {
                    return Err(ToolError::invalid("--build-with is for new projects; a project keeps the stack it was built with.")
                        .with_remediation("Leave out --build-with, or start a new project."));
                }
                if a.generate_assets {
                    return Err(
                        ToolError::invalid("--generate-assets is for new projects.").with_remediation("Use project-assets-allow for an existing project.")
                    );
                }
            }
            let place = match (&folder, &a.projects_dir) {
                (Some(f), _) => Place::At(f, existing),
                (None, Some(d)) => Place::In(d),
                (None, None) => Place::Projects,
            };
            let ws = rebuild::project::new_project(&settings, &a.target, &a.request, place, a.build_with.as_deref(), a.generate_assets, a.backend.as_deref())?;
            if folder.is_some() {
                engine.session().remember_project(&ws.root)?;
            }
            (ws, rebuild::project::attached_request(attached, &a.request))
        }
    };
    println!("Project: {}", ws.root.display());
    // The saved builder may be off or gone; say who builds once the run has started, so a failed
    // start doesn't claim an AI that never ran.
    let replacement = rebuild::builder_replacement(&hub.config(), &ws, a.backend.as_deref());
    // A follow-up shows in the project's feed as the person typed it.
    let shown = message.clone();
    let good_enough = engine.session().settings.build.good_enough;
    let root = ws.root.clone();
    // `--loop` runs the saved loop even when it's saved switched off; switching it off while it
    // runs (in the app, or a builder's `project_loop_set`) ends it, as in the app.
    let saved_off_at_start = ws.manifest.build_loop.as_ref().is_some_and(|s| !s.on);
    let mut build_loop = a.build_loop.then(|| {
        let settings = ws.manifest.build_loop.clone().unwrap_or_default();
        let state = LoopState::start(settings, now_ms(), loop_rules::fingerprint(&ws));
        println!("Build loop: {}.", state.status());
        state
    });
    let opts = RunOptions {
        backend: a.backend.clone(),
        message,
        shown,
        continuing: a.resume,
        max_rounds: build_loop.as_ref().map_or(a.rounds, |l| l.settings.rounds()),
        good_enough,
        beginner: !cli.json,
        clean_room: Some(engine.session().clean_room()),
        ..RunOptions::default()
    };
    let mut run = RebuildRun::start(&hub, ws, opts)?;
    if let Some(replacement) = replacement {
        eprintln!("{}", rebuild::replaced_builder_notice(&replacement.saved, &replacement.used));
    }
    loop {
        let (ok, paused, ending) = watch(&mut run, a, build_loop.as_mut())?;
        let Some(state) = build_loop.as_mut() else { return Ok(ok) };
        let end = match (ending, ok, paused) {
            (Some(why), _, _) => Some(why),
            (None, _, true) => Some(LoopEnd::Stopped),
            (None, false, _) => Some(LoopEnd::Failed),
            (None, true, false) => {
                let ws = Workspace::open(&root)?;
                // Settings changed while the run worked count from the next run, as in the app.
                let switched_off = match ws.manifest.build_loop.clone() {
                    Some(s) if !s.on && !saved_off_at_start => Some(LoopEnd::Stopped),
                    Some(s) => {
                        state.update(LoopSettings { on: true, ..s }, now_ms());
                        None
                    }
                    None => None,
                };
                switched_off.or_else(|| {
                    let unreported = state.tokens_unreported;
                    let next = state.after_run(now_ms(), loop_rules::fingerprint(&ws));
                    if !unreported && state.tokens_unreported && state.settings.tokens.on {
                        eprintln!("This builder doesn't say how many tokens it uses, so the token budget can't end the loop.");
                    }
                    match next {
                        Next::Run => None,
                        Next::End(why) => Some(why),
                    }
                })
            }
        };
        if let Some(why) = end {
            println!("{}", why.words());
            // A limit ending the loop is how it's meant to end, even when it stopped a run.
            return Ok(ok || matches!(why, LoopEnd::Runs(_) | LoopEnd::Tokens(_) | LoopEnd::Time(_) | LoopEnd::Stalled(_)));
        }
        println!("Build loop: {}. A fresh builder carries on.", state.status());
        let base = RunOptions {
            backend: a.backend.clone(),
            message: Some(state.settings.prompt().to_string()),
            shown: Some(format!("Build loop, run {}: a fresh builder carries on from the last one's handoff.", state.runs.saturating_add(1))),
            good_enough,
            beginner: !cli.json,
            clean_room: Some(engine.session().clean_room()),
            ..RunOptions::default()
        };
        let opts = loop_rules::run_options(base, &state.settings);
        run = RebuildRun::start(&hub, Workspace::open(&root)?, opts)?;
    }
}

/// Print a run's events and answer its questions until it ends: (ended well, paused, the loop
/// limit that stopped it mid-run).
fn watch(run: &mut RebuildRun, a: &RebuildArgs, mut build_loop: Option<&mut LoopState>) -> Result<(bool, bool, Option<LoopEnd>), ToolError> {
    let (mut ok, mut was_paused, mut ending) = (false, false, None);
    let mut line_open = false;
    loop {
        if INTERRUPTED.load(Ordering::SeqCst) {
            run.stop();
        }
        // A loop's stop time or token budget stops the run it's in.
        if ending.is_none()
            && let Some(why) = build_loop.as_deref().and_then(|l| l.over_limit_now(now_ms()))
        {
            ending = Some(why);
            run.stop();
        }
        for e in run.poll() {
            match e {
                RunEvent::Step { step, text } => {
                    end_line(&mut line_open);
                    println!("[{}] {text}", step.label());
                }
                RunEvent::Agent(AgentEvent::TextDelta { text }) => {
                    print!("{text}");
                    line_open = !text.ends_with('\n');
                }
                RunEvent::Agent(AgentEvent::ToolStarted { tool, input, .. }) => {
                    end_line(&mut line_open);
                    println!("  · {}", rebuild::activity::describe(&tool, &input, &[]).words());
                }
                RunEvent::Agent(AgentEvent::Error { message, remediation, log }) => {
                    end_line(&mut line_open);
                    println!("  ! {message}");
                    if let Some(r) = remediation {
                        println!("{r}");
                    }
                    if let Some(l) = log {
                        println!("  log: {}", l.display());
                    }
                }
                RunEvent::Agent(AgentEvent::TurnDone { input_tokens, output_tokens, .. }) => {
                    if let Some(l) = build_loop.as_deref_mut() {
                        l.add_tokens(input_tokens.saturating_add(output_tokens));
                    }
                }
                RunEvent::Agent(_) => {}
                RunEvent::Note { shown, .. } => {
                    end_line(&mut line_open);
                    println!("  + {shown}");
                }
                RunEvent::Ask(r) => {
                    end_line(&mut line_open);
                    let answer = if a.unattended { auto_answer(&r) } else { prompt_answer(&r) };
                    run.answer(&r.id, &answer)?;
                }
                RunEvent::Checked(c) => {
                    end_line(&mut line_open);
                    match c.similarity {
                        Some(s) => println!("[Check {}] {s:.0}% alike — {}", c.round, c.note),
                        None => println!("[Check {}] {}", c.round, c.note),
                    }
                }
                RunEvent::Preview(url) => println!("Preview: {url}"),
                RunEvent::Finished { ok: o, message, paused } => {
                    end_line(&mut line_open);
                    println!(
                        "{}",
                        if o {
                            format!("Done. {message}")
                        } else if paused {
                            message
                        } else {
                            format!("Stopped: {message}")
                        }
                    );
                    ok = o;
                    was_paused = paused;
                }
            }
        }
        if !run.is_running() && run.poll().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = std::io::stdout().flush();
    // Ctrl+C ends a loop as it ends a run.
    if INTERRUPTED.load(Ordering::SeqCst) {
        was_paused = true;
    }
    Ok((ok, was_paused, ending))
}

fn end_line(open: &mut bool) {
    if *open {
        println!();
        *open = false;
    }
}

fn auto_answer(r: &Request) -> Answer {
    match r.kind {
        RequestKind::Question => {
            println!("? {} → {} (unattended)", r.title, r.choices.first().map(String::as_str).unwrap_or("no preference"));
            Answer {
                choice: r.choices.first().cloned(),
                text: r.choices.is_empty().then(|| "No preference; use your best judgement.".to_string()),
                ..Default::default()
            }
        }
        RequestKind::Approval => {
            println!("? {} → refused (unattended)", r.title);
            Answer { allow: Some(false), text: Some("Unattended run: actions outside the project aren't allowed.".into()), ..Default::default() }
        }
    }
}

fn read_line() -> String {
    let mut s = String::new();
    let _ = std::io::stdin().lock().read_line(&mut s);
    s.trim().to_string()
}

fn prompt_answer(r: &Request) -> Answer {
    println!("\n? {}", r.title);
    if !r.detail.is_empty() {
        println!("  {}", r.detail.lines().take(12).collect::<Vec<_>>().join("\n  "));
    }
    match r.kind {
        RequestKind::Question => {
            for (i, c) in r.choices.iter().enumerate() {
                println!("  {}) {c}", i + 1);
            }
            print!("Your answer (number or text): ");
            let _ = std::io::stdout().flush();
            let line = read_line();
            match line.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| r.choices.get(i)) {
                Some(c) => Answer { choice: Some(c.clone()), ..Default::default() },
                None => Answer { text: Some(line), ..Default::default() },
            }
        }
        RequestKind::Approval => {
            print!("Allow? [y]es / [a]lways for this project / [n]o: ");
            let _ = std::io::stdout().flush();
            let line = read_line().to_lowercase();
            Answer { allow: Some(line.starts_with('y') || line.starts_with('a')), always: line.starts_with('a'), ..Default::default() }
        }
    }
}
