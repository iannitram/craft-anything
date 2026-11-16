//! Aider (`aider --message-file`).
//!
//! Aider reads positional arguments as files to edit, so the message goes to `--message-file`
//! (`--message` over SSH, where no file is written). The plain adapter passed it as a file name
//! and every run stopped at once (#72); a brief in argv passed Windows' command-line limit (#402).
//!
//! A one-shot run still asks questions and can open pages before the model is reached: it offers
//! to make a git repository and to add `.aider*` to `.gitignore`, checks for a new version, shows
//! release notes, asks about analytics and Playwright, and offers documentation URLs for model
//! warnings or a missing key. The flags below turn each of those off, and `BROWSER` is pointed at
//! a command that does nothing, so no page can open from a run. `--yes-always` is deliberately
//! not one of them: answering every confirmation with "yes" would also accept a page that the
//! person should decide on, and the confirmations a build needs already default to "yes" when
//! nobody can answer them.
//!
//! Before a run, [`check_setup`] refuses the two cases where Aider would wait five minutes for a
//! sign-in that nobody is there to finish: no model and no API key at all, and an OpenRouter model
//! with no OpenRouter key (#72).
//!
//! Outside a build the message may only read, so `--dry-run` stops Aider writing files. Its own
//! reads can't be checked, so a Clean room run is refused in `validate_access`, as for Grok (R2.2).

use super::*;
use std::path::{Path, PathBuf};

/// What stops Aider asking, updating, or offering to open a page.
const QUIET: &[&str] = &[
    "--no-check-update",
    "--no-show-release-notes",
    "--no-pretty",
    "--no-fancy-input",
    "--no-git",
    "--no-gitignore",
    "--no-analytics",
    "--no-detect-urls",
    "--no-suggest-shell-commands",
    "--no-show-model-warnings",
    "--disable-playwright",
];

/// The keys Aider's own default-model picker looks for when no model is chosen.
const DEFAULT_MODEL_KEYS: &[&str] = &["OPENROUTER_API_KEY", "ANTHROPIC_API_KEY", "DEEPSEEK_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY", "VERTEXAI_PROJECT"];

/// The key file Aider's own OpenRouter sign-in writes, in the home folder.
const SIGN_IN_KEYS: &str = ".aider/oauth-keys.env";

/// The most of a settings file we read while checking.
const SETTINGS_FILE_CAP: u64 = 1024 * 1024;

/// Aider's model shortcut flags; each one chooses the model, so none needs `--model`.
const MODEL_SHORTCUTS: &[&str] = &[
    "--opus",
    "--sonnet",
    "--haiku",
    "--4",
    "-4",
    "--4o",
    "--mini",
    "--4-turbo",
    "--35turbo",
    "--35-turbo",
    "--3",
    "-3",
    "--deepseek",
    "--o1-mini",
    "--o1-preview",
];

pub struct Aider;

/// Saved extra arguments without a message in them: `--message`, `--msg` and `-m` as Aider's
/// parser declares them, followed by the message or joined to it (`--msg=…`, `-m…`). argparse
/// also takes `--ms` for `--msg`; every shorter `--mes…` is shared with `--message-file`, so
/// Aider refuses it (checked against 0.86.2's options). Its short flags without a value, `-v` and
/// `-h`, may come first in one group: `-vm task` is `-v -m task`.
fn without_message(args: &[String]) -> Vec<String> {
    let mut kept = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        if matches!(arg.as_str(), "--message" | "--msg" | "--ms") {
            it.next();
        } else if arg.starts_with("--message=") || arg.starts_with("--msg=") || arg.starts_with("--ms=") {
            // Joined to its message: nothing of it is kept.
        } else if let Some(group) = arg.strip_prefix('-').filter(|g| !g.starts_with('-')) {
            let (flags, after) = group.split_at(group.len() - group.trim_start_matches(['v', 'h']).len());
            // Python 3.10's argparse also reads `-v=m` as `-v -m` (3.11 on refuses it).
            let rest = after.strip_prefix('=').filter(|_| flags.len() == 1).unwrap_or(after);
            match rest.strip_prefix('m') {
                Some(joined) => {
                    if joined.is_empty() {
                        it.next();
                    }
                    if !flags.is_empty() {
                        kept.push(format!("-{flags}"));
                    }
                }
                None => kept.push(arg.clone()),
            }
        } else {
            kept.push(arg.clone());
        }
    }
    kept
}

impl AgentAdapter for Aider {
    fn ids(&self) -> &'static [&'static str] {
        &["aider"]
    }

    fn args(&self, cx: &ArgsCx) -> Vec<String> {
        let mut a = Vec::new();
        if let Some(m) = &cx.options.cli_model {
            a.extend(["--model".into(), m.clone()]);
        }
        a.extend(QUIET.iter().copied().map(str::to_string));
        if !cx.builder {
            a.push("--dry-run".into());
        }
        match cx.message_file {
            // Aider runs `--message` before it reads `--message-file`, so a saved one would
            // replace the request.
            Some(_) => a.extend(without_message(&cx.options.args)),
            None => a.extend(cx.options.args.iter().cloned()),
        }
        // Last, so a saved extra argument can't be mistaken for the message.
        match cx.message_file {
            Some(file) => a.extend(["--message-file".into(), file.display().to_string()]),
            None => a.extend(["--message".into(), format!("{}{}", cx.preamble, cx.prompt)]),
        }
        a
    }

    fn reads_message_file(&self) -> bool {
        true
    }

    /// The message flag and the two that keep a run from asking about git or the console. Aider's
    /// plain form reads the prompt as a file name, so a failed check keeps this adapter rather
    /// than falling back to the plain one (#72).
    fn required_flags(&self) -> (&'static [&'static str], &'static [&'static str]) {
        (&["--help"], &["--message-file", "--message", "--no-fancy-input", "--no-git"])
    }

    fn plain_fallback(&self) -> bool {
        false
    }
    /// Aider reads whatever files it likes and never asks us, so a Clean room run can't hold it
    /// back. Grok is refused for the same reason (R2.2).
    fn validate_access(&self, access: Access) -> Result<(), ToolError> {
        match access {
            Access::CleanRoomAnalyse | Access::CleanRoomBuild | Access::CleanRoomQuestion => {
                Err(ToolError::invalid("Aider reads files on its own without asking, so a Clean room run can't hold it back.")
                    .with_remediation("Use a deep build for Aider. Its reads aren't checked, so it can't be kept out of the original's files."))
            }
            _ => Ok(()),
        }
    }

    fn streams_json(&self) -> bool {
        false
    }

    /// Refuse before Aider starts when it would stop at a sign-in nobody can finish (#72).
    fn check_native_options(&self, options: &BackendOptions, cwd: Option<&Path>) -> Result<(), ToolError> {
        check_setup(
            options,
            &|name| std::env::var(name).ok(),
            &Files::for_run(cwd, dirs::home_dir().as_deref(), flag_value(&options.args, "--env-file").as_deref()),
        )
    }

    fn check_remote_options(&self, options: &BackendOptions, host: &str, stop: &Stop) -> Result<(), ToolError> {
        stop.check()?;
        let stop = stop.clone();
        let args = crate::ssh::ssh_line_args(host, remote_setup_line(options));
        let out =
            match Process::new("ssh").args(&args).timeout(std::time::Duration::from_secs(15)).stdout_cap(1024).cancel_when(move || stop.is_stopped()).run() {
                Ok(out) => out,
                Err(e) if e.code == ErrorCode::Cancelled => return Err(e),
                Err(e) => {
                    // A check that can't run says nothing about Aider: let the run fail with ssh's
                    // own error, as it did before this check existed.
                    log::warn!("couldn't check Aider's setup on {host}: {}; starting the run anyway", e.message);
                    return Ok(());
                }
            };
        remote_setup_status(host, &out)
    }

    fn remote_run_env(&self) -> Option<(&'static str, String)> {
        Some(("BROWSER", "true".into()))
    }

    /// Nothing Aider offers to open is worth a page during a run, offline or not.
    fn process(&self, program: &str) -> Process {
        Process::new(program).env("BROWSER", "true")
    }
}

/// Determine only whether the SSH account can start Aider without browser sign-in. Shell
/// arguments contain booleans and the optional dotenv path, never the model or credential values.
/// The remote scan reads regular files only and reports its decision by exit status.
fn remote_setup_line(options: &BackendOptions) -> String {
    let model = configured_model(options, &|_| None, &Files::for_run(None, None, None));
    let mode = match model.as_deref() {
        Some(model) if model.starts_with("openrouter/") => "openrouter",
        Some(_) => "configured",
        None => "discover",
    };
    let openrouter = key_in_args(&options.args, "OPENROUTER_API_KEY");
    let any_key = DEFAULT_MODEL_KEYS.iter().any(|key| key_in_args(&options.args, key));
    let args = [
        mode,
        if openrouter { "1" } else { "0" },
        if any_key { "1" } else { "0" },
        &flag_value(&options.args, "--env-file").unwrap_or_default(),
        &SETTINGS_FILE_CAP.to_string(),
        &DEFAULT_MODEL_KEYS.join(" "),
    ];
    let script = crate::ssh::shell_quote(REMOTE_SETUP);
    let args = args.into_iter().map(crate::ssh::shell_quote).collect::<Vec<_>>().join(" ");
    format!("sh -c {script} aider-setup {args} </dev/null >/dev/null 2>&1")
}

const REMOTE_SETUP: &str = r#"
command -v awk >/dev/null && command -v wc >/dev/null || exit 12
# The SSH runner starts Aider in the login folder; ~/.env is also the default dotenv.
small_file() {
    [ -n "$1" ] || return 1
    [ -e "$1" ] || return 1
    [ -f "$1" ] || return 1
    # An existing settings file that can't be read or measured is an error, not "absent".
    [ -r "$1" ] || exit 13
    bytes=$(wc -c < "$1") || exit 13
    [ "$bytes" -le "$2" ]
}
# Keep the selected dotenv path literal, even if it begins with a dash.
file=$4
case "$file" in ""|/*) ;; *) file="./$file";; esac
env_file=/dev/null
home_env=/dev/null
oauth=/dev/null
conf=/dev/null
if small_file "$file" "$5"; then env_file=$file; fi
if small_file "$HOME/.env" "$5"; then home_env=$HOME/.env; fi
if small_file "$HOME/.aider/oauth-keys.env" "$5"; then oauth=$HOME/.aider/oauth-keys.env; fi
if small_file "$HOME/.aider.conf.yml" "$5"; then conf=$HOME/.aider.conf.yml; fi
awk -v mode="$1" -v router="$2" -v any="$3" -v keys="$6" '
function trim(s) {
    sub(/^[[:space:]]+/, "", s)
    sub(/[[:space:]]+$/, "", s)
    return s
}
function value(s) {
    s=trim(s)
    if (s ~ /^#/) return ""
    if (s !~ /^["\047]/) sub(/ #.*/, "", s)
    s=trim(s)
    gsub(/^["\047]+|["\047]+$/, "", s)
    return s
}
function dotenv(file,    line, key, v, eq, r) {
    while ((r = (getline line < file)) > 0) {
        line=trim(line)
        sub(/^export /, "", line)
        eq=index(line, "=")
        if (!eq) continue
        key=trim(substr(line, 1, eq-1))
        v=value(substr(line, eq+1))
        if (v != "") dot[key]=v
    }
    if (r < 0) failed=1
    close(file)
}
function config(file,    line, key, eq, r, items, n, i, p, parts) {
    while ((r = (getline line < file)) > 0) {
        if (line ~ /^model:/ && conf_model == "") conf_model=value(substr(line, 7))
        if (line ~ /^(api-key|set-env):[[:space:]]*\[/) {
            items=line
            sub(/^(api-key|set-env):[[:space:]]*\[/, "", items)
            sub(/\][[:space:]]*(#.*)?$/, "", items)
            n=split(items, parts, ",")
            for (i=1; i<=n; i++) {
                p=value(parts[i])
                eq=index(p, "=")
                if (eq && trim(substr(p, eq+1)) != "") configured[substr(p, 1, eq-1)]=1
            }
            continue
        }
        line=trim(line)
        sub(/^-+/, "", line)
        line=value(line)
        eq=index(line, "=")
        if (eq && trim(substr(line, eq+1)) != "") configured[substr(line, 1, eq-1)]=1
    }
    if (r < 0) failed=1
    close(file)
}
function has_key(key,    provider) {
    provider=key
    sub(/_API_KEY$/, "", provider)
    return dot[key] != "" || dot["AIDER_" key] != "" || trim(ENVIRON[key]) != "" ||
        trim(ENVIRON["AIDER_" key]) != "" || configured[key] || configured[tolower(provider)]
}
BEGIN {
    # Later files win, as Aider loads them: saved sign-in keys, the home .env, then --env-file.
    # Within one file the last assignment wins, as python-dotenv reads it.
    dotenv(ARGV[3]); dotenv(ARGV[2]); dotenv(ARGV[1]); config(ARGV[4])
    model=dot["AIDER_MODEL"]
    if (model == "") model=trim(ENVIRON["AIDER_MODEL"])
    if (model == "") model=conf_model
    if (mode == "openrouter" || (mode == "discover" && model ~ /^openrouter\//))
        exit (router || has_key("OPENROUTER_API_KEY")) ? 0 : 11
    if (mode == "configured" || model != "" || any) exit 0
    split(keys, defaults, " ")
    for (i in defaults) if (has_key(defaults[i])) exit 0
    exit 10
}' "$env_file" "$home_env" "$oauth" "$conf"
"#;

fn remote_setup_status(host: &str, out: &craft_anything_platform::Output) -> Result<(), ToolError> {
    if out.success {
        return Ok(());
    }
    // Only the two definite answers refuse. Anything else (missing awk or wc, an unreadable file,
    // no POSIX sh, a login shell that fails) says nothing about whether Aider would wait at a
    // sign-in, so the run goes ahead and fails later with ssh's or Aider's own error.
    match out.code {
        Some(10) => Err(ToolError::new(ErrorCode::BadSettings, format!("On {host}, Aider has no model or API key set up."))
            .with_remediation("Set a model or API key in Aider's ~/.env, ~/.aider.conf.yml or ~/.aider/oauth-keys.env on that computer.")),
        Some(11) => Err(ToolError::new(ErrorCode::BadSettings, format!("On {host}, Aider needs an OpenRouter API key for its selected model."))
            .with_remediation("Set OPENROUTER_API_KEY in ~/.env or ~/.aider/oauth-keys.env on that computer, or select another model.")),
        other => {
            let reason = match other {
                Some(12) => "awk or wc isn't available".to_string(),
                Some(13) => "a settings file couldn't be read".to_string(),
                Some(code) => format!("the check exited with code {code}"),
                None => "the check ended without an exit code".to_string(),
            };
            log::warn!("couldn't check Aider's setup on {host}: {reason}; starting the run anyway");
            Ok(())
        }
    }
}

/// The settings files Aider reads for a run, each list with the file it loads last (whose
/// settings win) first.
struct Files {
    env: Vec<PathBuf>,
    conf: Vec<PathBuf>,
}

impl Files {
    /// Aider looks in the folder it runs in, the top of its git repository (found from there even
    /// with `--no-git`), then the home folder. Its saved OpenRouter key loads before any `.env`.
    /// The folder is taken as Aider sees it, links resolved, so the repository is the same one.
    /// An `--env-file` in its arguments loads last of all, relative to that folder.
    fn for_run(cwd: Option<&Path>, home: Option<&Path>, env_file: Option<&str>) -> Files {
        let cwd = cwd.and_then(|d| d.canonicalize().or_else(|_| std::path::absolute(d)).ok());
        let git_top = cwd.as_deref().and_then(|d| d.ancestors().find(|a| is_repository_top(a)));
        let mut folders: Vec<&Path> = Vec::new();
        for folder in [cwd.as_deref(), git_top, home].into_iter().flatten() {
            if !folders.contains(&folder) {
                folders.push(folder);
            }
        }
        let named = env_file.map(Path::new).and_then(|f| if f.is_absolute() { Some(f.to_path_buf()) } else { cwd.as_deref().map(|c| c.join(f)) });
        let mut env: Vec<PathBuf> = named.into_iter().chain(folders.iter().map(|f| f.join(".env"))).collect();
        env.extend(home.map(|h| h.join(SIGN_IN_KEYS)));
        Files { conf: folders.iter().map(|f| f.join(".aider.conf.yml")).collect(), env }
    }
}

/// Whether `dir` is a git repository's top as GitPython finds it: a `.git` folder that holds a
/// `HEAD`, or a worktree's `.git` file.
fn is_repository_top(dir: &Path) -> bool {
    let git = dir.join(".git");
    git.is_file() || git.join("HEAD").is_file()
}

/// The model Aider will run: ours if chosen, else one its own arguments, environment or config
/// name. `None` means Aider would pick one from whichever key it finds, or offer a sign-in. A
/// `.env` comes before the environment, as Aider loads its `.env` files over it.
fn configured_model(options: &BackendOptions, env: &dyn Fn(&str) -> Option<String>, files: &Files) -> Option<String> {
    model_in_args(&options.args)
        .or_else(|| options.args.iter().find(|a| MODEL_SHORTCUTS.contains(&a.as_str())).cloned())
        .or_else(|| options.cli_model.as_deref().map(str::trim).filter(|m| !m.is_empty()).map(str::to_string))
        .or_else(|| files.env.iter().find_map(|file| env_file_value(&read_small(file)?, "AIDER_MODEL")))
        .or_else(|| env("AIDER_MODEL").map(|m| m.trim().to_string()).filter(|m| !m.is_empty()))
        .or_else(|| files.conf.iter().find_map(|file| model_in_conf(&read_small(file)?)))
}

/// The last `--model` in the extra arguments, since Aider's parser keeps the last one.
fn model_in_args(args: &[String]) -> Option<String> {
    flag_value(args, "--model")
}

/// The last value of `flag` in the extra arguments, as `flag value` or `flag=value`.
fn flag_value(args: &[String], flag: &str) -> Option<String> {
    let joined = format!("{flag}=");
    let mut found = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let value = if arg == flag { rest.next().map(String::as_str) } else { arg.strip_prefix(joined.as_str()) };
        if let Some(value) = value {
            let value = value.trim();
            found = (!value.is_empty()).then(|| value.to_string());
        }
    }
    found
}

/// A value without a trailing ` # comment`. Quoted values are left alone.
fn strip_comment(value: &str) -> &str {
    let value = value.trim();
    if value.starts_with(['"', '\'']) {
        return value;
    }
    if value.starts_with('#') {
        return "";
    }
    value.split_once(" #").map_or(value, |(v, _)| v).trim()
}

/// A top-level `model:` in Aider's own config file.
fn model_in_conf(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let value = strip_comment(line.strip_prefix("model:")?).trim_matches(['"', '\'']);
        (!value.is_empty()).then(|| value.to_string())
    })
}

/// Whether `name` is set by Aider's own `--api-key provider=KEY` or `--set-env NAME=VALUE`.
fn key_in_args(args: &[String], name: &str) -> bool {
    args.iter().enumerate().any(|(i, arg)| {
        // Aider's dedicated flags carry the key itself: --openai-api-key KEY or --openai-api-key=KEY.
        let flag = arg.split_once('=').map_or(arg.as_str(), |(flag, _)| flag);
        if let Some(provider) = flag.strip_prefix("--").and_then(|f| f.strip_suffix("-api-key")).filter(|p| !p.is_empty()) {
            let key = match arg.split_once('=') {
                Some((_, key)) => key,
                None => args.get(i + 1).map(String::as_str).unwrap_or_default(),
            };
            return format!("{}_API_KEY", provider.to_ascii_uppercase()) == name && !key.trim().is_empty();
        }
        let (flag, value) = if let Some(v) = arg.strip_prefix("--api-key=") {
            ("--api-key", v)
        } else if let Some(v) = arg.strip_prefix("--set-env=") {
            ("--set-env", v)
        } else if arg == "--api-key" || arg == "--set-env" {
            (arg.as_str(), args.get(i + 1).map(String::as_str).unwrap_or_default())
        } else {
            return false;
        };
        let Some((key, value)) = value.split_once('=') else { return false };
        let key = if flag == "--api-key" { format!("{}_API_KEY", key.trim().to_ascii_uppercase()) } else { key.trim().to_string() };
        key == name && !value.trim().is_empty()
    })
}

/// Whether a key is set in the environment (bare or `AIDER_`-prefixed), in Aider's arguments,
/// or in one of its key or config files.
fn key_seen(name: &str, options: &BackendOptions, env: &dyn Fn(&str) -> Option<String>, files: &Files) -> bool {
    let prefixed = format!("AIDER_{name}");
    let names = [name, prefixed.as_str()];
    if names.iter().any(|n| env(n).is_some_and(|v| !v.trim().is_empty())) || key_in_args(&options.args, name) {
        return true;
    }
    files.env.iter().any(|file| read_small(file).is_some_and(|text| names.iter().any(|n| env_file_value(&text, n).is_some())))
        || files.conf.iter().any(|file| read_small(file).is_some_and(|text| conf_sets_key(&text, name)))
}

/// The value of `NAME=value` in a dotenv file, with or without `export` and around quotes.
fn env_file_value(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let line = line.trim();
        let line = line.strip_prefix("export ").unwrap_or(line);
        let (key, value) = line.split_once('=')?;
        let value = strip_comment(value).trim_matches(['"', '\'']);
        (key.trim() == name && !value.is_empty()).then(|| value.to_string())
    })
}

/// A `provider=KEY` or `NAME=VALUE` list item in Aider's config (`api-key:` / `set-env:`).
fn conf_sets_key(text: &str, name: &str) -> bool {
    let provider = name.strip_suffix("_API_KEY").unwrap_or(name).to_ascii_lowercase();
    let sets = |item: &str| {
        let item = item.trim().trim_matches(['"', '\'']);
        [name, provider.as_str()].iter().any(|n| item.strip_prefix(*n).and_then(|rest| rest.strip_prefix('=')).is_some_and(|v| !v.trim().is_empty()))
    };
    text.lines().any(|line| {
        let line = line.trim();
        if let Some(list) = line.strip_prefix("api-key:").or_else(|| line.strip_prefix("set-env:")) {
            let list = list.trim().strip_prefix('[').unwrap_or(list.trim());
            let list = list.split(']').next().unwrap_or(list);
            return list.split(',').any(sets);
        }
        sets(line.trim_start_matches('-').trim())
    })
}

/// A file only if it's small enough to read while checking, so a huge one can't stall a run.
fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    (meta.is_file() && meta.len() <= SETTINGS_FILE_CAP).then(|| std::fs::read_to_string(path).ok()).flatten()
}

/// Refuse when Aider would offer its OpenRouter sign-in, which opens a page and waits five minutes
/// for an answer nobody can give. Otherwise a run is left to Aider's own error for that key.
fn check_setup(options: &BackendOptions, env: &dyn Fn(&str) -> Option<String>, files: &Files) -> Result<(), ToolError> {
    match configured_model(options, env, files) {
        None if DEFAULT_MODEL_KEYS.iter().any(|key| key_seen(key, options, env, files)) => Ok(()),
        None => Err(ToolError::new(
            ErrorCode::BadSettings,
            "Aider has no model or API key set up, so it would offer to sign in to OpenRouter and wait for a browser nobody is at.",
        )
        .with_remediation(
                        "Set OPENROUTER_API_KEY, ANTHROPIC_API_KEY or OPENAI_API_KEY for the computer Craft Anything runs on, or name a model in Aider's own ~/.aider.conf.yml. Aider's ~/.env counts too.",
        )),
        Some(model) if model.starts_with("openrouter/") && !key_seen("OPENROUTER_API_KEY", options, env, files) => Err(ToolError::new(
            ErrorCode::BadSettings,
            format!("Aider's model {model} needs an OpenRouter API key, and without one it would offer to sign in to OpenRouter and wait for a browser nobody is at."),
        )
        .with_remediation("Set OPENROUTER_API_KEY (in the environment, ~/.env or ~/.aider/oauth-keys.env), or choose a model from another provider for Aider.")),
        Some(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(builder: bool, options: &BackendOptions) -> Vec<String> {
        args_with(builder, options, None)
    }

    fn args_with(builder: bool, options: &BackendOptions, message_file: Option<&Path>) -> Vec<String> {
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        Aider.args(&ArgsCx {
            options,
            prompt: "make a game",
            preamble: "PRE ",
            mcp: &mcp,
            resume: None,
            builder,
            reads_checked: false,
            question: false,
            message_file,
        })
    }

    #[test]
    fn a_saved_message_cannot_replace_the_request_in_its_file() {
        let saved = |args: &[&str]| BackendOptions { args: args.iter().map(|a| a.to_string()).collect(), ..Default::default() };
        let file = Path::new("message.txt");
        for kept in [
            &["--architect", "--message", "old task"][..],
            &["--msg=old task", "--architect"],
            &["--ms", "old task", "--architect"],
            &["--ms=old task", "--architect"],
            &["-m", "old task", "--architect"],
            &["-mold task", "--architect"],
        ] {
            let a = args_with(true, &saved(kept), Some(file));
            assert!(!a.iter().any(|x| x.contains("old task") || x == "--message" || x == "-m"), "{kept:?}: {a:?}");
            assert!(a.contains(&"--architect".to_string()), "other saved arguments stay: {a:?}");
            assert_eq!(&a[a.len() - 2..], ["--message-file".to_string(), file.display().to_string()]);
        }
        // In a group after flags that take no value: the message goes, the flags stay.
        // Python 3.10 also reads `-v=m` as `-v -m`.
        for group in
            [&["-vm", "old task", "--architect"][..], &["-vhmold task", "--architect"], &["-v=m", "old task", "--architect"], &["-v=mold task", "--architect"]]
        {
            let a = args_with(true, &saved(group), Some(file));
            assert!(!a.iter().any(|x| x.contains("old task") || x.contains('m') && x.starts_with('-') && !x.starts_with("--")), "{group:?}: {a:?}");
            assert!(a.contains(&"--architect".to_string()), "{a:?}");
            assert!(a.iter().any(|x| x.starts_with("-v")), "the other flags stay: {a:?}");
        }
        // A short option that isn't the message, or takes a value of its own, is kept whole.
        let a = args_with(true, &saved(&["-c", "my.yml", "-cm.yml", "-v"]), Some(file));
        assert!(a.windows(4).any(|w| w == ["-c", "my.yml", "-cm.yml", "-v"]), "{a:?}");
        // With the message in argv it comes last, so it already wins over a saved one.
        let a = args(true, &saved(&["--message", "old task"]));
        assert_eq!(&a[a.len() - 2..], ["--message", "PRE make a game"]);
    }

    #[test]
    fn a_written_message_goes_in_by_its_file() {
        let file = Path::new("message.txt");
        let a = args_with(true, &BackendOptions::default(), Some(file));
        assert_eq!(&a[a.len() - 2..], ["--message-file".to_string(), file.display().to_string()]);
        assert!(!a.iter().any(|x| x == "--message" || x.contains("make a game")), "the message stays off the command line: {a:?}");
        assert!(Aider.reads_message_file());
    }

    /// A check with no keys in the environment and no home folder, as on a fresh computer.
    fn no_keys(options: &BackendOptions) -> Result<(), ToolError> {
        check_setup(options, &|_| None, &at_home(None))
    }

    /// The files of a run outside any project, with this home folder.
    fn at_home(home: Option<&Path>) -> Files {
        Files::for_run(None, home, None)
    }

    /// Run the on-host setup script exactly as `check_remote_options` does, with a fixture home
    /// and only the environment a remote shell would see.
    #[cfg(unix)]
    fn run_remote_setup(home: &Path, options: &BackendOptions, env: &[(&str, &str)]) -> craft_anything_platform::Output {
        let mut proc = Process::new("sh")
            .args(["-c", &remote_setup_line(options)])
            .cwd(home)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home.as_os_str())
            .timeout(std::time::Duration::from_secs(5));
        for (key, value) in env {
            proc = proc.env(key, value);
        }
        proc.run().unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn the_remote_setup_check_uses_host_settings_and_keeps_credentials_there() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let bare = BackendOptions::default();
        let result = run_remote_setup(home, &bare, &[]);
        assert_eq!(result.code, Some(10));
        let error = remote_setup_status("fixture", &result).unwrap_err();
        assert!(error.message.contains("On fixture"));
        assert!(result.stdout.is_empty() && result.stderr.is_empty(), "only the exit status crosses");

        std::fs::write(home.join(".aider.conf.yml"), "model: openrouter/fixture\n").unwrap();
        assert_eq!(run_remote_setup(home, &bare, &[]).code, Some(11));
        std::fs::write(home.join(".aider.conf.yml"), "model: openrouter/fixture\napi-key: [\"openrouter=sk-test\"] # mine\n").unwrap();
        assert!(run_remote_setup(home, &bare, &[]).success, "a flow-style api-key list on the host counts");
        std::fs::write(home.join(".aider.conf.yml"), "model: openrouter/fixture\n").unwrap();

        let oauth = home.join(".aider");
        std::fs::create_dir(&oauth).unwrap();
        std::fs::write(oauth.join("oauth-keys.env"), "OPENROUTER_API_KEY=sk-test\n").unwrap();
        assert!(run_remote_setup(home, &bare, &[]).success);
        std::fs::remove_file(oauth.join("oauth-keys.env")).unwrap();

        std::fs::write(home.join(".env"), "OPENROUTER_API_KEY=sk-test # fixture\n").unwrap();
        assert!(run_remote_setup(home, &bare, &[]).success);
        std::fs::remove_file(home.join(".env")).unwrap();
        std::fs::remove_file(home.join(".aider.conf.yml")).unwrap();

        // The host's exported environment counts, not only the files.
        assert!(run_remote_setup(home, &bare, &[("OPENAI_API_KEY", "sk-test")]).success);
        assert!(run_remote_setup(home, &bare, &[("AIDER_MODEL", "ollama/fixture")]).success);
        assert_eq!(run_remote_setup(home, &bare, &[("AIDER_MODEL", "openrouter/fixture")]).code, Some(11));
        assert!(run_remote_setup(home, &bare, &[("AIDER_MODEL", "openrouter/fixture"), ("OPENROUTER_API_KEY", "sk-test")]).success);

        let path = "-override 'name'.env";
        std::fs::write(home.join(path), "AIDER_MODEL=ollama/fixture\n").unwrap();
        let explicit_file = BackendOptions { args: vec!["--env-file".into(), path.into()], ..Default::default() };
        assert!(run_remote_setup(home, &explicit_file, &[]).success);

        let explicit_key = BackendOptions { args: vec!["--api-key".into(), "openrouter=sk-test".into()], ..Default::default() };
        assert!(!remote_setup_line(&explicit_key).contains("sk-test"));
        assert!(run_remote_setup(home, &explicit_key, &[]).success);
        let dedicated = BackendOptions { args: vec!["--openrouter-api-key".into(), "sk-test".into()], ..Default::default() };
        assert!(!remote_setup_line(&dedicated).contains("sk-test"));
        assert!(run_remote_setup(home, &dedicated, &[]).success, "Aider's dedicated provider flag counts on the host");
        assert_eq!(Aider.remote_run_env(), Some(("BROWSER", "true".into())));
    }

    /// python-dotenv keeps the last assignment in one file, and a named `--env-file` loads after
    /// the home `.env`; both decide the remote status.
    #[cfg(unix)]
    #[test]
    fn the_remote_setup_check_keeps_the_last_dotenv_assignment_and_the_named_file_last() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let bare = BackendOptions::default();
        std::fs::write(home.join(".env"), "AIDER_MODEL=openrouter/fixture\nAIDER_MODEL=ollama/fixture\n").unwrap();
        assert!(run_remote_setup(home, &bare, &[]).success, "the last assignment decides");

        let path = "override.env";
        std::fs::write(home.join(path), "AIDER_MODEL=gpt-4o\n").unwrap();
        let named = BackendOptions { args: vec!["--env-file".into(), path.into()], ..Default::default() };
        assert!(run_remote_setup(home, &named, &[]).success, "the named file loads after the home .env");
        std::fs::write(home.join(path), "AIDER_MODEL=openrouter/fixture\n").unwrap();
        assert_eq!(run_remote_setup(home, &named, &[]).code, Some(11), "the named file's OpenRouter model needs its key");
    }

    #[test]
    fn a_stopped_run_does_not_start_the_remote_check() {
        let stop = Stop::default();
        stop.stop();
        let err = Aider.check_remote_options(&BackendOptions::default(), "fixture.invalid", &stop).unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_settings_file_does_not_block_the_run() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let settings = home.join(".env");
        std::fs::write(&settings, "OPENAI_API_KEY=sk-test\n").unwrap();
        std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_to_string(&settings).is_ok() {
            // Root can read a 000 file; the check can't be exercised there.
            return;
        }
        let result = run_remote_setup(home, &BackendOptions::default(), &[]);
        std::fs::set_permissions(&settings, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(result.code, Some(13), "a present file that can't be read isn't absent");
        assert!(remote_setup_status("fixture", &result).is_ok(), "a check that can't read a file lets the run start");
    }

    /// A check that couldn't run says nothing about a sign-in, so only 10 and 11 refuse.
    #[test]
    fn only_the_two_definite_setup_answers_refuse() {
        let output = |code: i32| craft_anything_platform::Output {
            code: Some(code),
            success: false,
            stdout: Vec::new(),
            stderr: Vec::new(),
            stdout_tail: Vec::new(),
            stderr_tail: Vec::new(),
            stdout_truncated: false,
            stderr_truncated: false,
            duration: std::time::Duration::ZERO,
        };
        for code in [10, 11] {
            assert_eq!(remote_setup_status("fixture", &output(code)).unwrap_err().code, ErrorCode::BadSettings, "code {code}");
        }
        for code in [12, 13, 1, 255] {
            assert!(remote_setup_status("fixture", &output(code)).is_ok(), "code {code} must not refuse");
        }
    }

    #[test]
    fn the_message_goes_to_message_after_the_quiet_flags() {
        let a = args(true, &BackendOptions::default());
        assert_eq!(&a[a.len() - 2..], ["--message", "PRE make a game"]);
        for flag in ["--no-check-update", "--no-show-release-notes", "--no-git", "--no-gitignore", "--no-analytics", "--no-fancy-input"] {
            assert!(a.contains(&flag.to_string()), "{flag} is missing: {a:?}");
        }
        assert!(!a.iter().any(|x| x == "--yes-always"), "yes-always would also accept a sign-in: {a:?}");
        assert!(!a.contains(&"--dry-run".to_string()), "a build may write: {a:?}");
    }

    #[test]
    fn a_chat_without_a_build_cannot_write() {
        assert!(args(false, &BackendOptions::default()).contains(&"--dry-run".to_string()));
    }

    #[test]
    fn a_saved_model_and_extra_args_come_before_the_message() {
        let options = BackendOptions { cli_model: Some("openrouter/anthropic/claude-sonnet-4".into()), args: vec!["--architect".into()], ..Default::default() };
        let a = args(true, &options);
        assert_eq!(&a[..2], ["--model", "openrouter/anthropic/claude-sonnet-4"]);
        assert!(a.contains(&"--architect".to_string()));
        assert_eq!(&a[a.len() - 2..], ["--message", "PRE make a game"]);
    }

    #[test]
    fn aider_is_registered_and_never_run_plain() {
        assert!(AgentAdapters::built_in().for_agent("aider").ids().contains(&"aider"), "registered, not the plain adapter");
        assert_eq!(Aider.required_flags(), (&["--help"][..], &["--message-file", "--message", "--no-fancy-input", "--no-git"][..]));
        assert!(!Aider.plain_fallback() && !Aider.streams_json());
    }

    #[test]
    fn a_run_with_no_model_and_no_key_is_refused_before_aider_offers_a_sign_in() {
        let err = no_keys(&BackendOptions::default()).unwrap_err();
        assert_eq!(err.code, ErrorCode::BadSettings);
        assert!(err.message.contains("sign in to OpenRouter"), "{}", err.message);
        let fix = err.remediation.unwrap_or_default();
        assert!(fix.contains("~/.aider.conf.yml") && fix.contains("OPENAI_API_KEY"), "{fix}");
    }

    #[test]
    fn a_chosen_model_runs_and_a_missing_key_is_aiders_own_error() {
        let options = BackendOptions { cli_model: Some("anthropic/claude-sonnet-4".into()), ..Default::default() };
        assert!(no_keys(&options).is_ok(), "a model is enough to start; Aider reports a missing key plainly");
    }

    #[test]
    fn a_key_alone_lets_aider_pick_its_own_model() {
        let env = |name: &str| (name == "OPENAI_API_KEY").then(|| "sk-test".to_string());
        assert!(check_setup(&BackendOptions::default(), &env, &at_home(None)).is_ok());
    }

    #[test]
    fn an_openrouter_model_needs_an_openrouter_key() {
        let options = BackendOptions { cli_model: Some("openrouter/anthropic/claude-sonnet-4".into()), ..Default::default() };
        let err = no_keys(&options).unwrap_err();
        assert!(err.message.contains("OpenRouter API key"), "{}", err.message);
        assert!(err.remediation.unwrap_or_default().contains("OPENROUTER_API_KEY"));
        let env = |name: &str| (name == "OPENROUTER_API_KEY").then(|| "sk-test".to_string());
        assert!(check_setup(&options, &env, &at_home(None)).is_ok());
    }

    #[test]
    fn a_model_and_key_in_aiders_own_files_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".aider.conf.yml"), "# my settings\nmodel: gemini/gemini-2.5-pro\nauto-commits: false\n").unwrap();
        assert!(check_setup(&BackendOptions::default(), &|_| None, &at_home(Some(dir.path()))).is_ok(), "a model in Aider's config is a model");

        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".aider")).unwrap();
        std::fs::write(dir.path().join(".aider/oauth-keys.env"), "OPENROUTER_API_KEY=\"sk-test\"\n").unwrap();
        let options = BackendOptions { cli_model: Some("openrouter/anthropic/claude-sonnet-4".into()), ..Default::default() };
        assert!(check_setup(&options, &|_| None, &at_home(Some(dir.path()))).is_ok(), "Aider's saved sign-in key counts");
    }

    #[test]
    fn a_key_passed_in_aiders_own_arguments_counts() {
        let options = BackendOptions { args: vec!["--api-key".into(), "openrouter=sk-test".into()], ..Default::default() };
        assert!(key_in_args(&options.args, "OPENROUTER_API_KEY"));
        assert!(check_setup(&options, &|_| None, &at_home(None)).is_ok(), "a key in the arguments is enough for Aider to pick a model");
        let options = BackendOptions { args: vec!["--openai-api-key".into(), "sk-test".into()], ..Default::default() };
        assert!(key_in_args(&options.args, "OPENAI_API_KEY"), "a dedicated provider flag counts");
        assert!(check_setup(&options, &|_| None, &at_home(None)).is_ok());
        let options =
            BackendOptions { args: vec!["--openrouter-api-key=sk-test".into(), "--model=openrouter/anthropic/claude-sonnet-4".into()], ..Default::default() };
        assert!(key_in_args(&options.args, "OPENROUTER_API_KEY"), "a joined dedicated flag counts");
        assert!(check_setup(&options, &|_| None, &at_home(None)).is_ok(), "a dedicated key covers an OpenRouter model");
        let options = BackendOptions { args: vec!["--openai-api-key=".into()], ..Default::default() };
        assert!(!key_in_args(&options.args, "OPENAI_API_KEY"), "an empty dedicated value doesn't count");
        let options = BackendOptions { args: vec!["--model=anthropic/claude-sonnet-4".into()], ..Default::default() };
        assert_eq!(model_in_args(&options.args).as_deref(), Some("anthropic/claude-sonnet-4"));
        assert!(check_setup(&options, &|_| None, &at_home(None)).is_ok());
    }

    #[test]
    fn a_model_shortcut_flag_counts_as_a_model() {
        let options = BackendOptions { args: vec!["--sonnet".into()], ..Default::default() };
        assert!(no_keys(&options).is_ok(), "a shortcut flag chooses the model");
    }

    #[test]
    fn an_aider_prefixed_variable_counts_as_a_key() {
        let env = |name: &str| (name == "AIDER_OPENAI_API_KEY").then(|| "sk-test".to_string());
        assert!(check_setup(&BackendOptions::default(), &env, &at_home(None)).is_ok());
    }

    #[test]
    fn a_model_or_key_in_the_home_dotenv_or_config_list_counts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".env"), "AIDER_MODEL=gpt-4o\n").unwrap();
        assert!(check_setup(&BackendOptions::default(), &|_| None, &at_home(Some(dir.path()))).is_ok());

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".aider.conf.yml"), "api-key:\n  - openai=sk-test\n").unwrap();
        assert!(check_setup(&BackendOptions::default(), &|_| None, &at_home(Some(dir.path()))).is_ok());

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".aider.conf.yml"), "api-key: [openai=sk-test, anthropic=sk-test]\n").unwrap();
        assert!(check_setup(&BackendOptions::default(), &|_| None, &at_home(Some(dir.path()))).is_ok(), "a flow-style api-key list counts");
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".aider.conf.yml"), "model: openrouter/fixture\napi-key: [\"openrouter=sk-test\"] # mine\n").unwrap();
        let openrouter = BackendOptions { cli_model: Some("openrouter/fixture".into()), ..Default::default() };
        assert!(check_setup(&openrouter, &|_| None, &at_home(Some(dir.path()))).is_ok(), "a quoted flow item and its comment count");
    }

    /// A project's own settings count, where Aider itself reads them: the folder it runs in and the
    /// top of its git repository.
    #[test]
    fn a_model_or_key_in_the_projects_own_files_counts() {
        let (home, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let app = project.path().join("app");
        std::fs::create_dir_all(&app).unwrap();
        let check = |options: &BackendOptions| check_setup(options, &|_| None, &Files::for_run(Some(&app), Some(home.path()), None));
        assert!(check(&BackendOptions::default()).is_err(), "nothing set up anywhere");
        std::fs::write(app.join(".aider.conf.yml"), "model: gpt-4o\n").unwrap();
        assert!(check(&BackendOptions::default()).is_ok(), "a model in the run's folder");

        let openrouter = BackendOptions { cli_model: Some("openrouter/anthropic/claude-sonnet-4".into()), ..Default::default() };
        assert!(check(&openrouter).is_err());
        std::fs::write(project.path().join(".env"), "OPENROUTER_API_KEY=sk-test\n").unwrap();
        assert!(check(&openrouter).is_err(), "not a git repository, so Aider doesn't read its top");
        std::fs::create_dir(project.path().join(".git")).unwrap();
        assert!(check(&openrouter).is_err(), "an empty .git folder isn't a repository to GitPython");
        std::fs::write(project.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        assert!(check(&openrouter).is_ok(), "a key at the top of the git repository");
        std::fs::remove_dir_all(project.path().join(".git")).unwrap();
        std::fs::write(project.path().join(".git"), "gitdir: /elsewhere/.git/worktrees/app\n").unwrap();
        assert!(check(&openrouter).is_ok(), "a worktree's .git file marks its top too");
        assert!(check_setup(&openrouter, &|_| None, &at_home(Some(home.path()))).is_err(), "a run without a folder reads only the home folder");
    }

    /// The file Aider loads last decides the model: the run's folder over the home folder.
    #[test]
    fn the_projects_model_wins_over_the_home_folders() {
        let (home, app) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let check = || check_setup(&BackendOptions::default(), &|_| None, &Files::for_run(Some(app.path()), Some(home.path()), None));
        std::fs::write(home.path().join(".aider.conf.yml"), "model: openrouter/anthropic/claude-sonnet-4\n").unwrap();
        std::fs::write(app.path().join(".aider.conf.yml"), "model: gpt-4o\n").unwrap();
        assert!(check().is_ok(), "the project's model needs no OpenRouter key");
        std::fs::write(home.path().join(".aider.conf.yml"), "model: gpt-4o\n").unwrap();
        std::fs::write(app.path().join(".aider.conf.yml"), "model: openrouter/anthropic/claude-sonnet-4\n").unwrap();
        assert!(check().unwrap_err().message.contains("OpenRouter API key"), "the project's OpenRouter model needs its key");
    }

    /// Aider loads its `.env` files over the environment, so a model there wins over one set
    /// outside it.
    #[test]
    fn a_dotenv_model_wins_over_the_environments() {
        let app = tempfile::tempdir().unwrap();
        std::fs::write(app.path().join(".env"), "AIDER_MODEL=openrouter/anthropic/claude-sonnet-4\n").unwrap();
        let env = |name: &str| match name {
            "AIDER_MODEL" => Some("gpt-4o".to_string()),
            "OPENAI_API_KEY" => Some("sk-test".to_string()),
            _ => None,
        };
        let err = check_setup(&BackendOptions::default(), &env, &Files::for_run(Some(app.path()), None, None)).unwrap_err();
        assert!(err.message.contains("OpenRouter API key"), "{}", err.message);
    }

    /// An `--env-file` in Aider's arguments loads after every default `.env`, so it wins.
    #[test]
    fn a_named_env_file_wins_over_the_default_ones() {
        let app = tempfile::tempdir().unwrap();
        std::fs::write(app.path().join(".env"), "AIDER_MODEL=gpt-4o\n").unwrap();
        std::fs::write(app.path().join(".env.aider"), "AIDER_MODEL=openrouter/anthropic/claude-sonnet-4\n").unwrap();
        let options = BackendOptions { args: vec!["--env-file".into(), ".env.aider".into()], ..Default::default() };
        let files = Files::for_run(Some(app.path()), None, flag_value(&options.args, "--env-file").as_deref());
        assert!(check_setup(&options, &|_| None, &files).unwrap_err().message.contains("OpenRouter API key"));
        assert!(check_setup(&options, &|_| None, &Files::for_run(Some(app.path()), None, None)).is_ok(), "without it the project's .env decides");
    }

    /// A project reached through a link is read where it really is, with its real repository.
    #[cfg(unix)]
    #[test]
    fn a_linked_project_finds_its_real_repository() {
        let (real, links) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let app = real.path().join("app");
        std::fs::create_dir_all(real.path().join(".git")).unwrap();
        std::fs::write(real.path().join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
        std::fs::create_dir(&app).unwrap();
        std::fs::write(real.path().join(".aider.conf.yml"), "model: gpt-4o\n").unwrap();
        std::os::unix::fs::symlink(&app, links.path().join("app")).unwrap();
        assert!(check_setup(&BackendOptions::default(), &|_| None, &Files::for_run(Some(&links.path().join("app")), None, None)).is_ok());
    }

    #[test]
    fn a_clean_room_run_is_refused_and_other_modes_are_not() {
        for access in [Access::CleanRoomAnalyse, Access::CleanRoomBuild, Access::CleanRoomQuestion] {
            let err = Aider.validate_access(access).expect_err("a clean room can't hold Aider back");
            assert!(err.remediation.unwrap_or_default().contains("deep build"));
        }
        for access in [Access::Build, Access::Analyse, Access::Question] {
            assert!(Aider.validate_access(access).is_ok());
        }
    }

    #[test]
    fn an_inline_comment_is_not_part_of_a_value() {
        assert_eq!(model_in_conf("model: gpt-4o # my favourite\n").as_deref(), Some("gpt-4o"));
        assert_eq!(model_in_conf("model: # none yet\n"), None);
        assert_eq!(env_file_value("AIDER_MODEL=gpt-4o # note\n", "AIDER_MODEL").as_deref(), Some("gpt-4o"));
        assert_eq!(env_file_value("AIDER_MODEL= # unset\n", "AIDER_MODEL"), None);
    }
}
