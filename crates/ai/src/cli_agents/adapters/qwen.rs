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

    /// Extra arguments could ask for blanket approval (`-y`, `--yolo`, `--approval-mode yolo`),
    /// which would stop Qwen asking and so skip the Approvals menu. A model goes through the
    /// backend's model option instead ([`acp::ACP_MODEL_FLAGS`]).
    fn acp_options(&self, _options: &BackendOptions, _access: Access) -> Vec<String> {
        Vec::new()
    }

    fn validate_options(&self, options: &BackendOptions) -> Result<(), ToolError> {
        if options.args.is_empty() {
            Ok(())
        } else {
            Err(ToolError::invalid("Extra Qwen Code arguments could bypass Craft Anything's approval checks.")
                .with_remediation("Remove this backend's extra arguments. Set its program and model through the backend options instead."))
        }
    }

    /// Qwen asks before each tool call only in its default approval mode. Saying so here
    /// overrides an `approvalMode` the person's own `~/.qwen/settings.json` sets, so every
    /// request comes back to the Approvals menu.
    fn acp_mode_options(&self, _options: &BackendOptions, _access: Access) -> Vec<String> {
        vec!["--approval-mode".into(), "default".into()]
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
        assert!(c.args().iter().any(|a| a == "--experimental-acp"), "{:?}", c.args());
        assert!(c.args().windows(2).any(|w| w == ["--approval-mode", "default"]), "{:?}", c.args());
        // An explicit opt-out or an SSH host fails closed instead of running one-shot.
        spec.options.acp = Some(false);
        assert!(super::super::acp_for(&spec).is_none());
    }

    #[test]
    fn extra_qwen_arguments_are_refused_so_approvals_cant_be_bypassed() {
        let adapter = Qwen;
        assert!(adapter.validate_options(&BackendOptions::default()).is_ok());
        for args in [vec!["--yolo"], vec!["-y"], vec!["--approval-mode", "yolo"]] {
            let options = BackendOptions { args: args.iter().map(|a| a.to_string()).collect(), ..BackendOptions::default() };
            assert!(adapter.validate_options(&options).is_err(), "{args:?} must be refused");
            assert!(adapter.acp_options(&options, Access::Build).is_empty(), "{args:?} must never reach qwen");
        }
    }

    #[test]
    fn a_chosen_qwen_model_is_passed_over_acp() {
        assert_eq!(super::super::acp::model_args("qwen-code", Some("qwen3-coder-plus")), vec!["-m", "qwen3-coder-plus"]);
    }
}
