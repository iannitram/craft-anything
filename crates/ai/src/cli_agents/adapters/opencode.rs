//! OpenCode (`opencode run`). Without `run` its first argument names the folder to open, so a
//! prompt given on its own was read as a folder and every run stopped at once (#32).

use super::*;

pub struct OpenCode;

impl AgentAdapter for OpenCode {
    fn ids(&self) -> &'static [&'static str] {
        &["opencode"]
    }

    /// OpenCode V2 exposes a catalogue through its authenticated CLI.
    fn has_model_catalog(&self) -> bool {
        true
    }

    fn check_tool_server(&self, stop: &CancelToken) -> Result<(), ToolError> {
        cli_can_start(craft_anything_platform::self_cli_path().as_deref(), stop)
    }

    /// `opencode run` takes no MCP flag, and V2 ignores `OPENCODE_CONFIG_CONTENT`: the server
    /// goes in a config file named by `OPENCODE_CONFIG`, which the private (`--standalone`)
    /// server merges over the person's own settings. The background service never sees it.
    fn mcp_env(&self, mcp: &McpCommand) -> Result<Option<(&'static str, String)>, ToolError> {
        let content = config_content(mcp);
        let path = std::env::temp_dir().join(config_name(&content));
        craft_anything_platform::fs::write_atomic_private(&path, content.as_bytes())?;
        Ok(Some(("OPENCODE_CONFIG", path.to_string_lossy().into_owned())))
    }

    /// The remote helper is the same `craft-anything-cli` the local check starts; a build must
    /// not spend hours then find the remote helper missing. A missing program stops with the fix.
    fn check_remote_tool_server(&self, host: &str, stop: &CancelToken) -> Result<(), ToolError> {
        remote_check(host, Some(stop))
    }

    /// The config file is written on the remote host itself: one `ssh` invocation checks the
    /// helper, writes the file atomically and keeps it private (piped `printf`, octal-escaped,
    /// no interactive stdin). The private (`--standalone`) server reads it there.
    fn remote_tool_server(&self, host: &str, mcp: &McpCommand) -> Result<Option<(&'static str, String)>, ToolError> {
        let content = config_content(mcp);
        let name = config_name(&content);
        let line = remote_prepare_line(&name, &content);
        let output = ssh_run(host, &line, Duration::from_secs(30), None)?;
        if !output.success {
            return Err(remote_failure(host, &output));
        }
        Ok(Some(("OPENCODE_CONFIG", format!("~/.cache/craft-anything/{name}"))))
    }

    /// Discover enabled chat models and thinking variants from the selected local or SSH installation.
    fn model_catalog(
        &self,
        config: &BackendConfig,
        options: &BackendOptions,
        directory: Option<&Path>,
        stop: &Stop,
    ) -> Result<Vec<models::AgentModel>, ToolError> {
        models::discover(config, options, directory, stop)
    }

    /// Validate explicit choices against the live catalogue and refuse conflicting extra model arguments.
    /// With no selection, leave extra arguments to OpenCode and skip the catalogue query.
    fn validate_model(&self, config: &BackendConfig, options: &BackendOptions, directory: Option<&Path>, stop: &Stop) -> Result<(), ToolError> {
        if options.cli_model.is_none() && options.cli_thinking.is_none() {
            return Ok(());
        }
        // Extra flags must not silently override the selected model or its thinking variant.
        if options.args.iter().any(|a| a == "--model" || a.starts_with("--model=") || a.starts_with("-m")) {
            return Err(ToolError::invalid("Choose OpenCode's model in its model setting, rather than in extra arguments."));
        }
        models::validate(options, &self.model_catalog(config, options, directory, stop)?)
    }

    /// Build native `run` arguments, encoding an optional thinking depth as `provider/model#variant`.
    fn args(&self, cx: &ArgsCx) -> Vec<String> {
        // A private server reads this run's `OPENCODE_CONFIG`; the background service would not.
        let mut a = vec!["run".to_string(), "--standalone".to_string()];
        // A builder works unattended: what OpenCode would stop to ask is allowed, unless its own
        // settings deny it (as Codex gets a workspace-write sandbox).
        if cx.builder {
            a.push("--auto".into());
        }
        if let Some(m) = &cx.options.cli_model {
            let model = cx.options.cli_thinking.map_or_else(|| m.clone(), |d| format!("{m}#{}", d.as_str()));
            a.extend(["--model".into(), model]);
        }
        a.extend(cx.options.args.iter().cloned());
        a.extend(["--format".into(), "json".into()]);
        a.push(format!("{}{}", cx.preamble, cx.prompt));
        a
    }

    fn required_flags(&self) -> (&'static [&'static str], &'static [&'static str]) {
        (&["run", "--help"], &["--format", "--standalone"])
    }

    fn plain_fallback(&self) -> bool {
        false
    }

    /// A turn stopped part-way stays `running` in OpenCode's storage, and its shared service picks
    /// it up the next time it starts and runs the builder's commands again (#1115, seen with 2.2.5).
    /// A private server deletes it without resuming it. Short: the turn waits for it before it ends.
    fn discard_unfinished(&self, program: &str, id: &str, host: Option<&str>) -> Result<(), ToolError> {
        // From the agent's output: one read as a flag would delete nothing, or something else.
        if id.is_empty() || id.starts_with('-') {
            return Err(ToolError::invalid(format!("OpenCode named its session {id:?}, which can't be passed to `opencode session delete`."))
                .with_remediation("Find it with `opencode session list` and delete it with `opencode session delete --standalone <id>`."));
        }
        // With --standalone, as here: without it, the command may start the shared service, which resumes it.
        // Said for every failure (a time limit here isn't a setting the person can raise).
        let on = host.map(|h| format!(" on {h}")).unwrap_or_default();
        let by_hand = format!("Delete it yourself{on} with `opencode session delete --standalone {id}`, so OpenCode doesn't pick it up again.");
        let args = ["session", "delete", "--standalone", id].map(String::from);
        let proc = match host {
            Some(h) => Process::new("ssh").args(crate::ssh::ssh_line_args(h, crate::cli_agents::remote_command(program, args.to_vec(), &[]))),
            None => self.process(program).args(args),
        };
        let out = proc.timeout(std::time::Duration::from_secs(30)).stdout_cap(64 * 1024).run().map_err(|e| e.with_remediation(by_hand.clone()))?;
        if out.success {
            Ok(())
        } else {
            Err(ToolError::new(
                ErrorCode::ProgramFailed,
                format!("opencode session delete exited with {}", out.code.map_or("no code".into(), |c| c.to_string())),
            )
            .with_remediation(by_hand))
        }
    }

    fn discards_sessions(&self) -> bool {
        true
    }

    /// OpenCode 1.x has no `--standalone` and rejects it, so it's refused before it runs (#1111).
    fn missing_flags_error(&self) -> Option<(&'static str, &'static str)> {
        Some((
            "This OpenCode's `run --help` doesn't list --standalone, so it looks like OpenCode 1.x. Craft Anything needs OpenCode 2.",
            "Remove the old `opencode-ai` package (`npm uninstall -g opencode-ai`), then install OpenCode 2 (`npm install -g @opencode/cli`), or choose the OpenCode 2 program in Settings → AI.",
        ))
    }

    fn parse(&self, p: &mut StreamParser, v: &Value) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        if p.run_session.is_none() {
            p.run_session = v.get("sessionID").and_then(Value::as_str).map(str::to_string);
        }
        let part = v.get("part").unwrap_or(&Value::Null);
        match v.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    events.push(AgentEvent::TextDelta { text: text.into() });
                }
            }
            Some("reasoning") => {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    events.push(AgentEvent::Status { text: summary_of(text) });
                }
            }
            Some("tool_use") => {
                // V1 names the call `callID` (its `id` is the part's); V2 names it `id` (beside `partID`).
                let call = part.get("callID").or_else(|| part.get("id"));
                if let (Some(id), Some(name), Some(state)) = (call.and_then(Value::as_str), part.get("tool").and_then(Value::as_str), part.get("state")) {
                    if !p.tool_names.contains_key(id) {
                        events.push(p.started(id, name, state.get("input").cloned().unwrap_or_else(|| json!({}))));
                    }
                    match state.get("status").and_then(Value::as_str) {
                        Some("completed") => events.push(p.finished(id, true, &content_text(state.get("output").unwrap_or(&Value::Null)))),
                        Some("error") => events.push(p.finished(id, false, &content_text(state.get("error").unwrap_or(&Value::Null)))),
                        _ => {}
                    }
                }
            }
            Some("step_finish") => {
                let tokens = part.get("tokens").unwrap_or(&Value::Null);
                let count = |path| tokens.pointer(path).and_then(Value::as_u64).unwrap_or(0);
                // OpenCode's step-finish counters exclude cache from input and reasoning from
                // output, so these components make the inclusive totals the feed records.
                p.input_tokens = p.input_tokens.saturating_add(count("/input")).saturating_add(count("/cache/read")).saturating_add(count("/cache/write"));
                p.output_tokens = p.output_tokens.saturating_add(count("/output")).saturating_add(count("/reasoning"));
                if !p.cost_incomplete {
                    p.cost_usd = part.get("cost").and_then(Value::as_f64).filter(|n| n.is_finite() && *n >= 0.0).and_then(|cost| {
                        let total = p.cost_usd.unwrap_or(0.0) + cost;
                        total.is_finite().then_some(total)
                    });
                    p.cost_incomplete = p.cost_usd.is_none();
                }
            }
            Some("error") => {
                let error = v.get("error").unwrap_or(&Value::Null);
                let why = error_text(error);
                p.failure = Some(opencode_failure(error));
                p.saw_result = true;
                events.push(AgentEvent::Error { message: format!("OpenCode stopped: {why}"), remediation: None, log: None });
            }
            _ => {}
        }
        events
    }
}

/// Our tool server's config for `mcp`: the bytes of the `OPENCODE_CONFIG` file (both local and
/// remote runs use the same content).
fn config_content(mcp: &McpCommand) -> String {
    let command: Vec<&str> = std::iter::once(mcp.command.as_str()).chain(mcp.args.iter().map(String::as_str)).collect();
    json!({"mcp": {MCP_NAME: {"type": "local", "command": command, "enabled": true}}}).to_string()
}

/// Named by its content, so runs with the same server share one file and none is left half-written.
fn config_name(content: &str) -> String {
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    std::hash::Hash::hash(content, &mut hash);
    format!("craft-anything-opencode-{:016x}.json", std::hash::Hasher::finish(&hash))
}

/// Every byte as an octal escape for a POSIX `printf` format string, so no byte in `bytes` can
/// end the quoted string; safe for any path in any locale.
fn octal_escape(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 4);
    for b in bytes {
        out.push('\\');
        out.push((((b / 64) & 7) + b'0') as char);
        out.push((((b / 8) & 7) + b'0') as char);
        out.push(((b & 7) + b'0') as char);
    }
    out
}

/// The remote shell line: the helper is on PATH, then the config file goes in atomically and
/// private (`printf` with octal escapes; no byte ends the quoted string). One line, no input.
fn remote_prepare_line(name: &str, content: &str) -> String {
    let escaped = octal_escape(content.as_bytes());
    format!(
        "command -v craft-anything-cli >/dev/null && mkdir -p \"$HOME/.cache/craft-anything\" && printf '{escaped}' > \"$HOME/.cache/craft-anything/{name}.tmp.$$\" && chmod 600 \"$HOME/.cache/craft-anything/{name}.tmp.$$\" && mv \"$HOME/.cache/craft-anything/{name}.tmp.$$\" \"$HOME/.cache/craft-anything/{name}\""
    )
}

/// OpenCode's JSON error carries the provider status and its own retry decision. In particular,
/// an unclassified exit or an unsupported CLI flag cannot become retryable from display text.
/// What an OpenCode error event says went wrong, for the person (#1225): its message; else its
/// name and HTTP status, with the start of the provider's response body; else its own fields. The
/// words "no error details" are only for an event with nothing in it.
fn error_text(error: &Value) -> String {
    /// A non-blank string value, trimmed.
    fn text(v: Option<&Value>) -> Option<&str> {
        v.and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
    }
    let data = error.get("data");
    if let Some(message) = text(data.and_then(|d| d.get("message"))) {
        return message.to_string();
    }
    let name = text(error.get("name"));
    let status = data.and_then(|d| d.get("statusCode")).and_then(Value::as_u64);
    match (name, status) {
        (Some(name), status) => {
            let head = status.map_or_else(|| name.to_string(), |s| format!("{name} (HTTP {s})"));
            match text(data.and_then(|d| d.get("responseBody"))) {
                Some(body) => format!("{head}: {}", shorten(body)),
                None => head,
            }
        }
        (None, _) if error.is_null() || error.as_object().is_some_and(serde_json::Map::is_empty) => "the service returned no error details".into(),
        (None, _) => shorten(&error.to_string()),
    }
}

/// The first 300 characters of `text`, with an ellipsis when cut.
fn shorten(text: &str) -> String {
    let mut out: String = text.chars().take(300).collect();
    if text.chars().count() > 300 {
        out.push('…');
    }
    out
}

fn opencode_failure(error: &Value) -> TurnFailure {
    let data = error.get("data").unwrap_or(&Value::Null);
    match error.get("name").and_then(Value::as_str) {
        Some("ProviderAuthError") => TurnFailure::new(FailureKind::Authentication),
        Some("APIError") => {
            let status = data.get("statusCode").and_then(Value::as_u64).and_then(|n| u16::try_from(n).ok());
            let mut failure = status.map_or_else(
                || TurnFailure::new(FailureKind::Unknown),
                |status| {
                    TurnFailure::from_api(&crate::http::status_error("OpenCode", status, data.get("responseBody").and_then(Value::as_str).unwrap_or_default()))
                },
            );
            if failure.kind == FailureKind::Unknown && data.get("isRetryable").and_then(Value::as_bool) == Some(true) {
                failure.kind = FailureKind::Transient;
            } else if failure.retryable() && data.get("isRetryable").and_then(Value::as_bool) == Some(false) {
                failure.kind = FailureKind::Unknown;
            }
            failure.retry_after_secs = data.get("responseHeaders").and_then(Value::as_object).and_then(|headers| {
                headers
                    .iter()
                    .find(|(name, _)| name.eq_ignore_ascii_case("retry-after"))
                    .and_then(|(_, value)| value.as_str())
                    .and_then(|n| n.trim().parse::<u64>().ok())
                    .map(|n| n.clamp(1, crate::failure::MAX_RETRY_WAIT))
            });
            failure
        }
        _ => TurnFailure::new(FailureKind::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(builder: bool, options: &BackendOptions) -> Vec<String> {
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        OpenCode.args(&ArgsCx { options, prompt: "make a game", preamble: "PRE ", mcp: &mcp, resume: None, builder, reads_checked: false, question: false })
    }

    #[test]
    fn the_prompt_goes_to_run_after_the_flags() {
        assert_eq!(args(false, &BackendOptions::default()), ["run", "--standalone", "--format", "json", "PRE make a game"]);
        let options =
            BackendOptions { cli_model: Some("anthropic/claude-sonnet-4-5".into()), args: vec!["--agent".into(), "build".into()], ..Default::default() };
        assert_eq!(
            args(true, &options),
            ["run", "--standalone", "--auto", "--model", "anthropic/claude-sonnet-4-5", "--agent", "build", "--format", "json", "PRE make a game"]
        );
        assert!(AgentAdapters::built_in().for_agent("opencode").ids().contains(&"opencode"), "registered, not the plain adapter");
    }

    #[test]
    fn structured_errors_select_retry_without_reading_english() {
        let error = |status, retryable| json!({"name":"APIError","data":{"message":"服务限流。Please retry in 19.05s.","statusCode":status,"isRetryable":retryable,"responseHeaders":{"Retry-After":"4"}}});
        let rate = opencode_failure(&error(429, true));
        assert_eq!(rate, TurnFailure { kind: FailureKind::RateLimited, retry_after_secs: Some(4) });
        for (status, kind) in
            [(401, FailureKind::Authentication), (402, FailureKind::Billing), (400, FailureKind::Configuration), (503, FailureKind::Transient)]
        {
            assert_eq!(opencode_failure(&error(status, true)).kind, kind);
        }
        assert!(!opencode_failure(&error(429, false)).retryable(), "an explicit nonretryable response is kept");
        assert!(!opencode_failure(&json!({"name":"UnknownError","data":{"message":"Please retry in 2s"}})).retryable());
        assert_eq!(opencode_failure(&json!({"name":"ProviderAuthError","data":{"message":"重新登录"}})).kind, FailureKind::Authentication);
        assert_eq!(opencode_failure(&json!({"name":"APIError","data":{"isRetryable":true}})).kind, FailureKind::Transient);
        let quota = json!({"name":"APIError","data":{"statusCode":429,"isRetryable":true,"responseBody":"{\"error\":{\"code\":\"insufficient_quota\"}}"}});
        assert_eq!(opencode_failure(&quota).kind, FailureKind::Billing);
    }

    #[test]
    fn json_output_keeps_text_tools_and_usage_without_ending_the_process_turn() {
        let mut p = StreamParser::new(Arc::new(OpenCode));
        let lines = [
            json!({"type":"step_start","sessionID":"reported-but-not-resumable","part":{}}),
            json!({"type":"text","part":{"text":"Built a page"}}),
            json!({"type":"tool_use","part":{"callID":"write-1","tool":"write","state":{"status":"completed","input":{"path":"index.html"},"output":"wrote the page"}}}),
            json!({"type":"step_finish","part":{"tokens":{"input":10,"output":5,"reasoning":2,"cache":{"read":3,"write":1}},"cost":0.01}}),
            json!({"type":"step_finish","sessionID":"another","part":{"tokens":{"input":1,"output":2},"cost":0.02}}),
        ];
        let events: Vec<_> = lines.iter().flat_map(|v| p.line(&v.to_string())).collect();
        // #1115: kept so an unfinished turn's session can be deleted, the first one it named.
        assert_eq!(p.run_session.as_deref(), Some("reported-but-not-resumable"));
        assert!(
            matches!(events.as_slice(), [AgentEvent::TextDelta { text }, AgentEvent::ToolStarted { tool, .. }, AgentEvent::ToolFinished { ok:true, .. }] if text=="Built a page" && tool=="write")
        );
        assert_eq!((p.input_tokens, p.output_tokens), (15, 9));
        assert_eq!(p.cost_usd, Some(0.03));
        assert!(p.session_id.is_none(), "the adapter does not pretend it can resume");
        let events = p.line(&json!({"type":"error","error":{"name":"APIError","data":{"message":"暂时失败","statusCode":429,"isRetryable":true}}}).to_string());
        assert!(matches!(events.as_slice(), [AgentEvent::Error { .. }]), "run_cli alone owns TurnDone");
        assert_eq!(p.failure.unwrap().kind, FailureKind::RateLimited);
    }

    /// #1225: an error event without a message says what it does have, and "no error details" only
    /// when it has nothing.
    #[test]
    fn an_error_event_without_a_message_says_what_it_has() {
        assert_eq!(
            error_text(&json!({"name":"APIError","data":{"statusCode":400,"responseBody":"Request is missing x-opencode-session"}})),
            "APIError (HTTP 400): Request is missing x-opencode-session"
        );
        assert_eq!(error_text(&json!({"name":"UnknownError"})), "UnknownError");
        assert_eq!(error_text(&json!({"name":"APIError","data":{"responseBody":"  "}})), "APIError");
        assert_eq!(error_text(&json!({"data":{"message":"  the model failed  "}})), "the model failed");
        assert_eq!(error_text(&Value::Null), "the service returned no error details");
        assert_eq!(error_text(&json!({})), "the service returned no error details");
        assert_eq!(error_text(&json!("plain")), "\"plain\"", "a plain error is shown, not hidden");
        let long = "x".repeat(400);
        let cut = error_text(&json!({"name":"APIError","data":{"responseBody":long}}));
        assert!(cut.starts_with("APIError: ") && cut.ends_with('…'));
        assert_eq!(cut.chars().count(), "APIError: ".len() + 300 + 1);
    }

    #[test]
    fn missing_invalid_and_overflowed_costs_stay_unknown_for_the_turn() {
        let step = |cost| json!({"type":"step_finish","part":{"cost":cost,"tokens":{}}}).to_string();
        for invalid in [Value::Null, json!(-1), json!("unknown")] {
            for unknown_first in [true, false] {
                let mut p = StreamParser::new(Arc::new(OpenCode));
                let costs = if unknown_first { [invalid.clone(), json!(0.02)] } else { [json!(0.02), invalid.clone()] };
                for cost in costs {
                    p.line(&step(cost));
                }
                p.line(&step(json!(0.01)));
                assert_eq!(p.cost_usd, None, "a later known step cannot fill an earlier gap");
            }
        }
        for unknown_first in [true, false] {
            let mut missing = StreamParser::new(Arc::new(OpenCode));
            if !unknown_first {
                missing.line(&step(json!(0.02)));
            }
            missing.line(r#"{"type":"step_finish","part":{"tokens":{}}}"#);
            missing.line(&step(json!(0.01)));
            assert_eq!(missing.cost_usd, None);
        }
        let mut overflow = StreamParser::new(Arc::new(OpenCode));
        overflow.line(&step(json!(f64::MAX)));
        overflow.line(&step(json!(f64::MAX)));
        overflow.line(&step(json!(0)));
        assert_eq!(overflow.cost_usd, None);
        let mut zero = StreamParser::new(Arc::new(OpenCode));
        zero.line(&step(json!(0)));
        zero.line(&step(json!(0)));
        assert_eq!(zero.cost_usd, Some(0.0), "an explicit zero is a known cost");
    }

    #[test]
    fn native_runs_get_the_tool_server_from_a_config_file() {
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        let (var, path) = OpenCode.mcp_env(&mcp).unwrap().unwrap();
        assert_eq!(var, "OPENCODE_CONFIG");
        let v: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(v["mcp"][MCP_NAME]["command"], json!(["craft-anything-cli", "mcp"]));
        let options = BackendOptions { cli_model: Some("test/model".into()), ..Default::default() };
        assert!(OpenCode.check_native_options(&options, None).is_ok());
    }

    #[test]
    fn the_tool_server_config_and_name_are_stable() {
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into(), "--clean-room".into()], notes: None };
        let content = config_content(&mcp);
        assert_eq!(content, config_content(&mcp));
        assert!(content.contains("craft-anything-cli"));
        assert!(content.contains("--clean-room"));
        assert_eq!(config_name(&content), config_name(&content));
        let other = config_content(&McpCommand { command: "other".into(), args: vec![], notes: None });
        assert_ne!(config_name(&content), config_name(&other));
    }

    #[test]
    fn octal_escapes_every_byte() {
        assert_eq!(octal_escape(&[]), "");
        assert_eq!(octal_escape(&[0x41]), "\\101");
        assert_eq!(octal_escape(&[0x00, 0x27, 0x5C]), "\\000\\047\\134");
    }

    /// Check private config creation and refusal without writes when the remote helper is absent.
    #[test]
    #[cfg(unix)]
    fn the_remote_line_runs_on_a_posix_shell() {
        use craft_anything_platform::process::Process;
        use std::os::unix::fs::{PermissionsExt, symlink};

        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let missing_bin = root.path().join("missing-bin");
        std::fs::create_dir(&bin).unwrap();
        std::fs::create_dir(&missing_bin).unwrap();
        // A fake helper on PATH, so `command -v craft-anything-cli` succeeds or fails on purpose.
        std::fs::write(bin.join("craft-anything-cli"), "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(bin.join("craft-anything-cli"), std::fs::Permissions::from_mode(0o755)).unwrap();
        // Keep both paths isolated from any real helper installed on the test machine.
        for utility in ["mkdir", "chmod", "mv"] {
            let installed = craft_anything_platform::which(utility).unwrap();
            let executable = installed.canonicalize().unwrap();
            symlink(&executable, bin.join(utility)).unwrap();
            symlink(&executable, missing_bin.join(utility)).unwrap();
        }

        let home = root.path().join("home");
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into(), "--clean-room".into()], notes: None };
        let content = config_content(&mcp);
        let name = config_name(&content);
        let line = remote_prepare_line(&name, &content);

        let run = |path: &Path, home: &Path| {
            Process::new("/bin/sh")
                .args(["-c", &line])
                .env_clear()
                .env("HOME", home)
                .env("PATH", path)
                .timeout(Duration::from_secs(5))
                .idle_timeout(Duration::from_secs(5))
                .stdout_cap(4096)
                .stderr_cap(4096)
                .run()
                .unwrap()
        };
        let ok = run(&bin, &home);
        assert!(ok.success, "{}", ok.stderr_text());
        let target = home.join(".cache/craft-anything").join(&name);
        assert_eq!(std::fs::read(&target).unwrap(), content.as_bytes());
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);

        // POSIX requires a nonzero lookup status, but shells differ on its exact value.
        // A fresh home proves the failed lookup stops before any config directory or file is written.
        let missing_home = root.path().join("missing-home");
        let missing = run(&missing_bin, &missing_home);
        assert!(!missing.success, "a missing helper must refuse setup");
        assert!(missing.code.is_some(), "the shell must exit normally");
        assert!(!missing_home.exists(), "a missing helper must not write anything");
    }

    #[test]
    fn the_remote_line_needs_no_input_and_checks_the_helper_first() {
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        let content = config_content(&mcp);
        let name = config_name(&content);
        let line = remote_prepare_line(&name, &content);
        assert!(line.starts_with("command -v craft-anything-cli "), "{line}");
        assert!(line.contains("mkdir -p \"$HOME/.cache/craft-anything\""), "{line}");
        assert!(line.contains(&format!("\"$HOME/.cache/craft-anything/{name}\"")), "{line}");
        assert!(line.contains("chmod 600"), "{line}");
        assert!(!line.contains('\n'), "{line}");
        // No byte in the content can end the printf format string: every octet is escaped.
        let full_escaped = octal_escape(content.as_bytes());
        assert_eq!(octal_escape("{}".as_bytes()), "\\173\\175");
        assert!(line.contains(&format!("printf '{full_escaped}'")), "{line}");
    }

    /// OpenCode V2 (2.2.5) names the call `id`, beside a `partID`; V1 named it `callID`.
    #[test]
    fn v2_tool_events_reach_the_feed() {
        let mut p = StreamParser::new(Arc::new(OpenCode));
        let line = json!({"type":"tool_use","part":{"partID":"prt_1","type":"tool","id":"call_1","tool":"shell","state":{"status":"completed","input":{"command":"echo one"},"output":"one"}}});
        let events = p.line(&line.to_string());
        assert!(
            matches!(events.as_slice(), [AgentEvent::ToolStarted { tool, .. }, AgentEvent::ToolFinished { ok: true, .. }] if tool == "shell"),
            "{events:?}"
        );
    }

    /// Every supported depth reaches native chat and builder runs in the selected model reference.
    #[test]
    fn a_session_id_that_reads_as_a_flag_is_refused_before_anything_runs() {
        // No such program: had it been started, the error would say it wasn't found.
        for (id, host) in [("", None), ("-h", None), ("--all", Some("studio"))] {
            let e = OpenCode.discard_unfinished("no-such-opencode-here", id, host).unwrap_err();
            assert_eq!(e.code, ErrorCode::BadInput, "{id:?}: {e:?}");
            assert!(e.remediation.unwrap_or_default().contains("--standalone"), "the manual command doesn't wake the shared service");
        }
        // A delete that couldn't run says how to do it by hand, not the process runner's own advice.
        let e = OpenCode.discard_unfinished("no-such-opencode-here", "ses_1", None).unwrap_err();
        assert_ne!(e.code, ErrorCode::BadInput, "{e:?}");
        assert!(e.remediation.as_deref().unwrap_or_default().contains("opencode session delete --standalone ses_1"), "{e:?}");
    }

    #[test]
    fn thinking_depth_is_part_of_the_selected_model_reference() {
        for depth in crate::Effort::ALL {
            let options = BackendOptions { cli_model: Some("test/model".into()), cli_thinking: Some(depth), ..Default::default() };
            for builder in [false, true] {
                let argv = args(builder, &options);
                let model = argv.iter().position(|a| a == "--model").unwrap();
                assert_eq!(argv[model + 1], format!("test/model#{}", depth.as_str()));
                assert_eq!(argv.last().unwrap(), "PRE make a game");
            }
        }
        let argv = args(false, &BackendOptions::default());
        assert!(!argv.iter().any(|a| a == "--model"));
    }

    /// Extra model arguments work without a saved selection, but cannot override an explicit model or depth.
    #[test]
    fn extra_model_arguments_are_checked_only_for_explicit_selections() {
        let config = BackendConfig {
            name: "OpenCode".into(),
            kind: crate::BackendKind::AgentCli,
            model: "opencode".into(),
            base_url: None,
            ssh_host: None,
            enabled: true,
        };
        for extra in [vec!["--model", "test/legacy"], vec!["--model=test/legacy"], vec!["-m", "test/legacy"], vec!["-mtest/legacy"]] {
            let options =
                BackendOptions { command: Some("missing-test-opencode".into()), args: extra.iter().map(|s| s.to_string()).collect(), ..Default::default() };
            assert!(OpenCode.validate_model(&config, &options, None, &Stop::default()).is_ok());
            let argv = args(false, &options);
            assert_eq!(argv.get(2..2 + extra.len()).unwrap(), extra.as_slice());
            for (model, thinking) in [(Some("test/selected"), None), (None, Some(crate::Effort::High)), (Some("test/selected"), Some(crate::Effort::High))] {
                let selected = BackendOptions { cli_model: model.map(str::to_string), cli_thinking: thinking, ..options.clone() };
                let error = OpenCode.validate_model(&config, &selected, None, &Stop::default()).unwrap_err();
                assert_eq!(error.code, craft_anything_evidence::ErrorCode::BadInput);
            }
        }
    }
}
