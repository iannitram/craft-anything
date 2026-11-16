//! Devin (`devin -p`).
//!
//! `-p/--print` answers once and exits; the prompt is a positional argument, so it goes last.
//! Plain `devin <prompt>` opens an interactive session instead of answering, so a failed flag
//! check keeps this adapter rather than falling back to the plain one.
//!
//! Build runs go over ACP instead (`acp.rs`): one `devin acp` process serves the conversation,
//! takes our MCP server in `session/new`, and its permission requests reach the approval policy.
//! Its sessions authenticate with the backend's stored key (`authenticate` with `meta.api_key`):
//! Devin intentionally doesn't read its own CLI sign-in over ACP. This print path uses whatever
//! the CLI itself is signed in with.

use super::*;

pub struct Devin;

impl AgentAdapter for Devin {
    fn ids(&self) -> &'static [&'static str] {
        &["devin"]
    }

    fn args(&self, cx: &ArgsCx) -> Vec<String> {
        let mut v = Vec::new();
        if let Some(m) = &cx.options.cli_model {
            v.extend(["--model".into(), m.clone()]);
        }
        v.extend(cx.options.args.iter().cloned());
        v.extend(["-p".into(), format!("{}{}", cx.preamble, cx.prompt)]);
        v
    }

    fn required_flags(&self) -> (&'static [&'static str], &'static [&'static str]) {
        (&["--help"], &["--print"])
    }

    /// Plain `devin <prompt>` opens an interactive session; a failed flag check keeps this adapter.
    fn plain_fallback(&self) -> bool {
        false
    }

    fn streams_json(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(options: &BackendOptions) -> Vec<String> {
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: None };
        Devin.args(&ArgsCx {
            options,
            prompt: "make a game",
            preamble: "PRE ",
            mcp: &mcp,
            resume: None,
            builder: true,
            reads_checked: false,
            question: false,
            message_file: None,
        })
    }

    #[test]
    fn the_prompt_goes_last_after_print_and_the_flags() {
        assert_eq!(args(&BackendOptions::default()), ["-p", "PRE make a game"]);
        let options = BackendOptions { cli_model: Some("swe-2".into()), args: vec!["--permission-mode".into(), "auto".into()], ..Default::default() };
        assert_eq!(args(&options), ["--model", "swe-2", "--permission-mode", "auto", "-p", "PRE make a game"]);
    }

    #[test]
    fn devin_is_registered_and_never_run_plain() {
        assert!(AgentAdapters::built_in().for_agent("devin").ids().contains(&"devin"), "registered, not the plain adapter");
        assert_eq!(Devin.required_flags(), (&["--help"][..], &["--print"][..]));
        assert!(!Devin.plain_fallback() && !Devin.streams_json());
    }
}
