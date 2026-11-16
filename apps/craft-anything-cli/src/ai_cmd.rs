//! `chat` and `ai list|detect|add|test`.

use crate::output::Format;
use crate::{Cli, INTERRUPTED, emit, fail, make_engine, out};
use clap::{Args, Subcommand};
use craft_anything_ai::{AgentEvent, AiHub, BackendConfig, BackendEntry, BackendKind, BackendOptions, Effort};
use craft_anything_evidence::ToolError;
use serde_json::{Value, json};
use std::io::{Read as _, Write as _};
use std::sync::atomic::Ordering;
use std::time::Duration;

#[derive(Debug, Args)]
pub struct ChatArgs {
    /// The question.
    pub question: String,
    /// File, app or folder to open first.
    #[arg(long)]
    pub target: Option<String>,
    /// Backend name (default: the lead backend).
    #[arg(long)]
    pub backend: Option<String>,
    /// Plain-words answers.
    #[arg(long)]
    pub beginner: bool,
}

#[derive(Debug, Subcommand)]
pub enum AiCommand {
    /// Configured backends.
    List,
    /// Look for keys, local servers, agent CLIs and SSH hosts.
    Detect,
    /// Add a backend.
    Add {
        name: String,
        /// anthropic, openai, openrouter, openai_compatible, agent_cli
        #[arg(long)]
        kind: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        base_url: Option<String>,
        #[arg(long)]
        ssh_host: Option<String>,
        /// Read the API key from this environment variable (keys are never taken on the command line).
        #[arg(long)]
        key_env: Option<String>,
        /// Read the API key from stdin.
        #[arg(long)]
        key_stdin: bool,
        /// Talk to this agent app over ACP (`agent_cli` kind; the agent must speak it, e.g. devin).
        #[arg(long)]
        acp: bool,
    },
    /// Remove a backend.
    Remove { name: String },
    /// Show, replace or forget the key saved in the app for an API backend (the key is never printed).
    Key {
        name: String,
        /// Read the new key from this environment variable.
        #[arg(long, conflicts_with_all = ["key_stdin", "forget"])]
        key_env: Option<String>,
        /// Read the new key from stdin.
        #[arg(long, conflicts_with_all = ["key_env", "forget"])]
        key_stdin: bool,
        /// Forget the saved key; the backend stays and the environment key is used again.
        #[arg(long, conflicts_with_all = ["key_env", "key_stdin"])]
        forget: bool,
    },
    /// Turn a backend on again.
    On { name: String },
    /// Turn a backend off: nothing uses it, and it keeps its settings and key.
    Off { name: String },
    /// Set the program an agent app runs, when it's installed where it isn't found (leave the
    /// path out to find it by its usual name again).
    Program { name: String, path: Option<String> },
    /// Set the folder Claude Code keeps its sign-in, settings and sessions in (its
    /// CLAUDE_CONFIG_DIR), so this AI uses another account: a full path or one under `~/`, on the
    /// computer it runs on. Leave the path out for its usual folder.
    Folder { name: String, path: Option<String> },
    /// Set the model an AI uses (Claude Code: the model it's asked for); leave it out to list the
    /// ones it can use. `default` for Claude Code passes no `--model`, so it uses the model its own
    /// settings name (which may be a custom model), rather than a built-in default.
    Model {
        name: String,
        model: Option<String>,
        /// OpenCode thinking variant; only those offered by the selected model are accepted.
        #[arg(long, value_parser = ["low", "medium", "high", "xhigh", "max"])]
        thinking: Option<String>,
        /// Use the agent's own default model (OpenCode's default; Claude Code's own settings).
        #[arg(long, conflicts_with_all = ["model", "thinking"])]
        default: bool,
    },
    /// Set how hard a backend thinks: low, medium, high, xhigh or max for the Claude API or Claude
    /// Code; none, minimal, low, medium, high or xhigh for an OpenAI-style API (Gemini: minimal
    /// to high), which is sent no
    /// level until one is set. `default` goes back to the usual; leave it out to see the choices.
    Effort { name: String, level: Option<String> },
    /// Show or change an API backend's requests per minute. Zero removes the limit.
    RequestRate { name: String, requests_per_minute: Option<u32> },
    /// Show or choose the tools a builder on an API backend is offered: auto, lean (building only)
    /// or full (every tool). Auto gives a local model the lean list below a 64k window and every
    /// tool from there; a paid service always gets every tool.
    Tools { name: String, list: Option<String> },
    /// Send a tiny request and report latency.
    Test { name: String },
    /// Choose the backend that builds (leave the name out to go back to automatic).
    Builder { name: Option<String> },
    /// Choose the backend that takes a game's own files out after the study, in Deep mode (an API
    /// backend such as Kimi K3 through OpenRouter; leave the name out to turn it off).
    Extractor { name: Option<String> },
}

fn hub(cli: &Cli) -> Result<AiHub, ToolError> {
    let hub = AiHub::new(make_engine(cli.project.as_deref(), cli.deep_for_run())?);
    // Before any command runs, while no session lock is held (#1225).
    if let Err(e) = hub.ensure_session_id() {
        log::warn!("couldn't save the OpenCode Go session id: {e}");
    }
    Ok(hub)
}

fn write_raw(s: &str) {
    let mut o = std::io::stdout().lock();
    let _ = o.write_all(s.as_bytes()).and_then(|_| o.flush());
}

/// Headless agent run: stream the answer to stdout, tool activity to stderr.
pub fn chat(cli: &Cli, a: &ChatArgs) -> u8 {
    let hub = match hub(cli) {
        Ok(h) => h,
        Err(e) => return fail(&e, cli.format()),
    };
    if let Some(t) = &a.target {
        let abs = match std::path::absolute(t) {
            Ok(p) => p,
            Err(e) => return fail(&ToolError::io(t, e), cli.format()),
        };
        if let Err(e) = hub.engine.call("core_target_open", json!({"path": abs}), crate::call_options(cli, &Default::default())) {
            return fail(&e, cli.format());
        }
    }
    let mut conv = match hub.start(a.backend.as_deref(), a.beginner) {
        Ok(c) => c,
        Err(e) => return fail(&e, cli.format()),
    };
    conv.send(&a.question);
    let jsonl = cli.format() == Format::Jsonl;
    let mut code = 0u8;
    let mut stopped = false;
    loop {
        if INTERRUPTED.load(Ordering::SeqCst) && !stopped {
            conv.stop();
            stopped = true;
        }
        let events = conv.poll();
        if events.is_empty() {
            if stopped && !conv.is_busy() {
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
            continue;
        }
        let mut done = false;
        for ev in events {
            if jsonl {
                out(&serde_json::to_string(&ev).unwrap_or_default());
            }
            match &ev {
                AgentEvent::TextDelta { text } if !jsonl => write_raw(text),
                AgentEvent::Status { text } if !jsonl => eprintln!("· {text}"),
                AgentEvent::ToolStarted { tool, .. } if !jsonl => eprintln!("→ {tool}"),
                AgentEvent::ToolFinished { tool, ok, summary, evidence_id, .. } if !jsonl => {
                    let ev = evidence_id.as_deref().map(|e| format!(" [{e}]")).unwrap_or_default();
                    eprintln!("{} {tool}: {summary}{ev}", if *ok { "✓" } else { "✗" });
                }
                AgentEvent::ConsentNeeded { call_id, title, explanation, .. } => {
                    if !jsonl {
                        eprintln!(
                            "! {title} needs your OK: {explanation} ({})",
                            if cli.allow_run { "allowed by --allow-run" } else { "declined; re-run with --allow-run to allow" }
                        );
                    }
                    conv.answer_consent(call_id, cli.allow_run);
                }
                AgentEvent::Error { message, remediation, log } => {
                    code = 1;
                    if !jsonl {
                        eprintln!("error: {message}");
                        if let Some(r) = remediation {
                            eprintln!("fix: {r}");
                        }
                        if let Some(l) = log {
                            eprintln!("log: {}", l.display());
                        }
                    }
                }
                AgentEvent::TurnDone { backend, input_tokens, output_tokens, tokens_unknown, cost_usd, cost_is_estimate } => {
                    if !jsonl {
                        let cost = cost_usd
                            .map(|c| if *cost_is_estimate { format!(", about ${c:.4} at API prices (an estimate, not a bill)") } else { format!(", ${c:.4}") })
                            .unwrap_or_default();
                        write_raw("\n");
                        let usage = if *tokens_unknown { "token count unknown".into() } else { format!("{input_tokens} in / {output_tokens} out") };
                        eprintln!("[{backend}: {usage}{cost}]");
                    }
                    done = true;
                }
                _ => {}
            }
        }
        if done {
            break;
        }
    }
    if stopped { 130 } else { code }
}

fn read_key(key_env: &Option<String>, key_stdin: bool) -> Result<Option<String>, String> {
    if let Some(var) = key_env {
        return std::env::var(var).map(Some).map_err(|_| format!("environment variable {var} is not set"));
    }
    if key_stdin {
        let mut s = String::new();
        std::io::stdin().take(64 * 1024).read_to_string(&mut s).map_err(|e| format!("reading the key from stdin: {e}"))?;
        let k = s.trim().to_string();
        return if k.is_empty() { Err("no key on stdin".into()) } else { Ok(Some(k)) };
    }
    Ok(None)
}

/// Dispatch AI configuration, discovery and model-selection commands through the shared hub.
/// Render results in the requested format and return an exit status for success or failure.
pub fn ai(cli: &Cli, c: &AiCommand, f: Format) -> u8 {
    let hub = match hub(cli) {
        Ok(h) => h,
        Err(e) => return fail(&e, f),
    };
    match c {
        AiCommand::List => {
            let v: Vec<Value> = hub
                .backends()
                .into_iter()
                .map(|b| {
                    let has_key = hub.has_key(&b.name);
                    let mut v = serde_json::to_value(&b).unwrap_or(Value::Null);
                    if let Some(m) = v.as_object_mut() {
                        m.insert("key_stored".into(), json!(has_key));
                    }
                    v
                })
                .collect();
            emit(&Value::Array(v), f);
            0
        }
        AiCommand::Detect => {
            emit(&serde_json::to_value(hub.detect()).unwrap_or(Value::Null), f);
            // On stderr, so the list on stdout stays what scripts read.
            for hint in hub.detect_hints() {
                eprintln!("note: {} {}", hint.message(), hint.remediation());
            }
            0
        }
        AiCommand::Add { name, kind, model, base_url, ssh_host, key_env, key_stdin, acp } => {
            let kind: BackendKind = match serde_json::from_value(json!(kind)) {
                Ok(k) => k,
                Err(_) => return crate::usage_error("--kind must be one of anthropic, openai, openrouter, openai_compatible, agent_cli"),
            };
            let key = match read_key(key_env, *key_stdin) {
                Ok(k) => k,
                Err(m) => return crate::usage_error(&m),
            };
            // A server by its address with no model named: asked for its models, and one that can chat is used.
            if let (BackendKind::OpenaiCompatible, Some(address), None) = (kind, base_url, model) {
                return match hub.add_server(name, address, None, key) {
                    Ok(model) => {
                        emit(&json!({"added": {"name": name, "kind": kind, "model": model, "base_url": address}}), f);
                        0
                    }
                    Err(e) => fail(&e, f),
                };
            }
            let model = model.clone().or_else(|| kind.suggested_models().first().map(|m| m.to_string())).unwrap_or_default();
            let cfg = BackendConfig { name: name.clone(), kind, model, base_url: base_url.clone(), ssh_host: ssh_host.clone(), enabled: true };
            let entry = BackendEntry { config: cfg.clone(), options: BackendOptions { acp: acp.then_some(true), ..Default::default() } };
            match hub.add_backend_with(entry, key) {
                Ok(()) => {
                    emit(&json!({"added": cfg}), f);
                    0
                }
                Err(e) => fail(&e, f),
            }
        }
        AiCommand::Remove { name } => match hub.remove_backend(name) {
            Ok(removed) => {
                emit(&json!({"removed": removed, "name": name}), f);
                if removed { 0 } else { 1 }
            }
            Err(e) => fail(&e, f),
        },
        AiCommand::Key { name, key_env, key_stdin, forget } => {
            if *forget {
                return match hub.remove_backend_key(name) {
                    Ok(()) => {
                        emit(&json!({"name": name, "key_saved": false, "changed": "forgotten"}), f);
                        0
                    }
                    Err(e) => fail(&e, f),
                };
            }
            if key_env.is_some() || *key_stdin {
                let key = match read_key(key_env, *key_stdin) {
                    Ok(Some(k)) => k,
                    Ok(None) => return crate::usage_error("--key-env or --key-stdin is needed to replace a key"),
                    Err(m) => return crate::usage_error(&m),
                };
                return match hub.set_backend_key(name, &key) {
                    Ok(()) => {
                        emit(&json!({"name": name, "key_saved": true, "changed": "replaced"}), f);
                        0
                    }
                    Err(e) => fail(&e, f),
                };
            }
            match hub.saved_backend_key(name) {
                Ok(saved) => {
                    emit(&json!({"name": name, "key_saved": saved}), f);
                    0
                }
                Err(e) => fail(&e, f),
            }
        }
        AiCommand::On { name } | AiCommand::Off { name } => {
            let on = matches!(c, AiCommand::On { .. });
            match hub.set_enabled(name, on) {
                Ok(found) => {
                    emit(&json!({"name": name, "enabled": on, "found": found}), f);
                    if found { 0 } else { 1 }
                }
                Err(e) => fail(&e, f),
            }
        }
        AiCommand::Model { name, model, thinking, default } if *default || thinking.is_some() => {
            let depth = thinking.as_deref().and_then(craft_anything_ai::Effort::parse);
            let model = if *default { None } else { model.clone().or_else(|| hub.options().get(name).and_then(|o| o.cli_model.clone())) };
            // Claude Code keeps no live catalogue, so `--default` is its "no --model" choice:
            // `set_model` clears the override directly instead of a catalogue selection.
            let claude_code = *default
                && hub.backends().iter().any(|b| {
                    b.name == *name && b.kind == BackendKind::AgentCli && craft_anything_ai::cli_agents::adapter_for(&b.model).ids().contains(&"claude-code")
                });
            if claude_code {
                return match hub.set_model(name, craft_anything_ai::anthropic::CLAUDE_CODE_DEFAULT) {
                    Ok(found) => {
                        emit(&json!({"name": name, "model": craft_anything_ai::anthropic::CLAUDE_CODE_DEFAULT, "found": found}), f);
                        if found { 0 } else { 1 }
                    }
                    Err(e) => fail(&e, f),
                };
            }
            match hub.set_agent_model(name, model.as_deref(), depth) {
                Ok(found) => {
                    emit(&json!({"name": name, "model": model, "thinking": depth, "found": found}), f);
                    if found { 0 } else { 1 }
                }
                Err(e) => fail(&e, f),
            }
        }
        AiCommand::Model { name, model: None, .. }
            if hub.model_context(name).is_ok_and(|c| {
                c.backend.config.kind == BackendKind::AgentCli && craft_anything_ai::cli_agents::adapter_for(&c.backend.config.model).has_model_catalog()
            }) =>
        {
            match hub.model_context(name).and_then(|c| hub.agent_models(&c)) {
                Ok(models) => {
                    emit(&json!({"name": name, "models": models}), f);
                    0
                }
                Err(e) => fail(&e, f),
            }
        }
        AiCommand::Model { name, model: None, .. } => match hub.model_choices(name) {
            Ok(models) => {
                emit(&json!({"name": name, "models": models}), f);
                0
            }
            Err(e) => fail(&e, f),
        },
        AiCommand::Model { name, model: Some(model), .. } => match hub.set_model(name, model) {
            Ok(found) => {
                emit(&json!({"name": name, "model": model.trim(), "found": found}), f);
                if found { 0 } else { 1 }
            }
            Err(e) => fail(&e, f),
        },
        AiCommand::Effort { name, level: None } => {
            let Some(b) = hub.backends().into_iter().find(|b| b.name == *name) else {
                return fail(&ToolError::invalid(format!("No AI backend named {name:?}")).with_remediation("See `craft-anything-cli ai list`."), f);
            };
            let options = hub.options();
            match craft_anything_ai::effort_levels(&b) {
                [] => fail(
                    &ToolError::invalid(format!("{name} doesn't take an effort level."))
                        .with_remediation("Effort is for the Claude API, Claude Code and OpenAI-style APIs."),
                    f,
                ),
                levels => {
                    let effort = craft_anything_ai::effort_in_use(&b, options.get(name)).map_or("default", Effort::as_str);
                    let levels: Vec<&str> = levels.iter().map(|e| e.as_str()).collect();
                    emit(&json!({"name": name, "effort": effort, "levels": levels}), f);
                    0
                }
            }
        }
        AiCommand::Effort { name, level: Some(level) } => {
            let effort = match level.trim() {
                "default" => None,
                l => match Effort::parse(l) {
                    Some(e) => Some(e),
                    None => {
                        let e = ToolError::invalid(format!("{l:?} isn't an effort level."))
                            .with_remediation("Use none, minimal, low, medium, high, xhigh, max or default.");
                        return fail(&e, f);
                    }
                },
            };
            match hub.set_effort(name, effort) {
                Ok(found) => {
                    emit(&json!({"name": name, "effort": effort.map(Effort::as_str), "found": found}), f);
                    if found { 0 } else { 1 }
                }
                Err(e) => fail(&e, f),
            }
        }
        AiCommand::RequestRate { name, requests_per_minute } => {
            let result = match requests_per_minute {
                Some(rate) => hub.set_requests_per_minute(name, *rate),
                None => hub.requests_per_minute(name),
            };
            match result {
                Ok(rate) => {
                    emit(&json!({"name": name, "requests_per_minute": rate}), f);
                    0
                }
                Err(e) => fail(&e, f),
            }
        }
        AiCommand::Tools { name, list } => {
            let result = match list.as_deref() {
                Some(text) => match craft_anything_ai::ToolList::parse(text) {
                    Some(list) => hub.set_tool_list(name, list).and_then(|_| hub.tool_list(name)),
                    None => Err(ToolError::invalid(format!("{text:?} isn't a tool list.")).with_remediation("Use auto, lean or full.")),
                },
                None => hub.tool_list(name),
            };
            match result {
                Ok((list, by_window)) => {
                    // What Auto works out to here, so the choice can be made beside it.
                    let auto = if by_window { "lean below a 64k window, full from there" } else { "full" };
                    emit(&json!({"name": name, "tools": list.as_str(), "auto": auto}), f);
                    0
                }
                Err(e) => fail(&e, f),
            }
        }
        AiCommand::Program { name, path } => match hub.set_command(name, path.as_deref()) {
            Ok(found) => {
                emit(&json!({"name": name, "program": path, "found": found}), f);
                if found { 0 } else { 1 }
            }
            Err(e) => fail(&e, f),
        },
        AiCommand::Folder { name, path } => match hub.set_config_dir(name, path.as_deref()) {
            Ok(found) => {
                emit(&json!({"name": name, "folder": path, "found": found}), f);
                if found { 0 } else { 1 }
            }
            Err(e) => fail(&e, f),
        },
        AiCommand::Builder { name } => match hub.set_role(craft_anything_ai::Role::Builder, name.as_deref()) {
            Ok(()) => {
                emit(&json!({"builder": name}), f);
                0
            }
            Err(e) => fail(&e, f),
        },
        AiCommand::Extractor { name } => match hub.set_role(craft_anything_ai::Role::Extractor, name.as_deref()) {
            Ok(()) => {
                emit(&json!({"extractor": name}), f);
                0
            }
            Err(e) => fail(&e, f),
        },
        AiCommand::Test { name } => match hub.test_backend(name) {
            Ok(r) => {
                emit(&serde_json::to_value(r).unwrap_or(Value::Null), f);
                0
            }
            Err(e) => fail(&e, f),
        },
    }
}

/// AI reachability checks for `doctor` (bounded; each configured backend once).
pub fn doctor_checks() -> Vec<craft_anything_setup::Check> {
    use craft_anything_setup::{Check, CheckStatus};
    let Ok(engine) = make_engine(None, None) else { return Vec::new() };
    let hub = AiHub::new(engine);
    let results = hub.check_reachability();
    if results.is_empty() {
        return vec![Check::new("ai", CheckStatus::Info, "No AI backend is set up (optional).").fix("Run `craft-anything-cli ai detect`, then `ai add`.")];
    }
    results
        .into_iter()
        .map(|(name, r)| match r {
            Ok(ms) => Check::new(format!("ai:{name}"), CheckStatus::Ok, format!("{name} answered in {ms} ms.")),
            Err(why) => Check::new(format!("ai:{name}"), CheckStatus::Warn, format!("{name} isn't reachable: {why}"))
                .fix(format!("Run `craft-anything-cli ai test {name}` for details.")),
        })
        .collect()
}
