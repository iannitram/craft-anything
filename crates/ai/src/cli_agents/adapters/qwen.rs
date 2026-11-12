//! Qwen Code, driven over ACP (`qwen --experimental-acp`). Run one-shot with no approval flag, its
//! own gate refuses every shell command it can't ask about, and the project's Approvals setting
//! never reached it. Over ACP the setting is applied per call and Qwen gets our tool server.

use super::*;

/// Qwen Code's ACP-only adapter. `acp = false` or an SSH host fails closed rather than running
/// the one-shot CLI, which has no approval channel back to Craft Anything.
pub struct Qwen;

impl AgentAdapter for Qwen {
    fn ids(&self) -> &'static [&'static str] {
        &["qwen-code", "qwen"]
    }

    fn args(&self, _cx: &ArgsCx) -> Vec<String> {
        // The native runner refuses `requires_acp` before it asks for arguments.
        Vec::new()
    }

    fn requires_acp(&self) -> bool {
        true
    }

    fn plain_fallback(&self) -> bool {
        false
    }

    fn check_tool_server(&self, stop: &CancelToken) -> Result<(), ToolError> {
        cli_can_start(craft_anything_platform::self_cli_path().as_deref(), stop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qwen_is_acp_only_and_registered_under_both_ids() {
        let adapters = AgentAdapters::built_in();
        for id in ["qwen-code", "qwen"] {
            let adapter = adapters.for_agent(id);
            assert!(adapter.ids().contains(&"qwen-code"), "{id} gets the Qwen adapter, not the plain one");
            assert!(adapter.requires_acp() && !adapter.plain_fallback());
        }
        assert_eq!(super::super::acp::acp_args("qwen-code"), Some(&["--experimental-acp"][..]));
    }

    #[test]
    fn a_qwen_builder_runs_over_acp_without_being_asked_to() {
        let config =
            BackendConfig { name: "Qwen".into(), kind: crate::BackendKind::AgentCli, model: "qwen-code".into(), base_url: None, ssh_host: None, enabled: true };
        let mut spec = super::super::CliSpec::new(config, BackendOptions::default(), false, None);
        let Some(c) = super::super::acp_for(&spec) else { panic!("Qwen Code runs over ACP by default") };
        assert!(c.args.iter().any(|a| a == "--experimental-acp"), "{:?}", c.args);
        // An explicit opt-out or an SSH host fails closed instead of running one-shot.
        spec.options.acp = Some(false);
        assert!(super::super::acp_for(&spec).is_none());
    }
}
