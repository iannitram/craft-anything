//! Regressions for `QODER_CONFIG_DIR` skill-directory overrides and Windows `~\` tilde-prefix
//! path expansion in `craft_anything_setup::clients`.

use craft_anything_setup::{CheckStatus, DoctorRequest, Host, Os, PlanRequest, apply, clients, doctor, plan, uninstall};
use std::collections::BTreeMap;
use std::path::Path;

/// `QODER_CONFIG_DIR` relocates Qoder's configuration folder, so `skills_dir` must follow that
/// override across `clients`, `plan`, `apply`, `doctor`, and `uninstall` instead of writing into
/// `<home>/.qoder/skills`.
#[test]
fn qoder_config_dir_override_moves_skills_with_settings() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let custom_qoder = dir.path().join("custom-qoder");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&custom_qoder).unwrap();

    let cli = dir.path().join(if cfg!(windows) { "craft-anything-cli.exe" } else { "craft-anything-cli" });
    std::fs::write(&cli, "").unwrap();

    let mut host = Host::with_home(&home);
    host.env.insert("QODER_CONFIG_DIR".into(), custom_qoder.display().to_string());

    let qoder = clients(&host).into_iter().find(|c| c.id == "qoder").expect("qoder client");
    assert_eq!(qoder.config, custom_qoder.join("settings.json"));
    assert_eq!(qoder.skills_dir, custom_qoder.join("skills"));

    let req = PlanRequest { clients: vec!["qoder".into()], all_detected: false, command: vec![cli.display().to_string(), "mcp".into()], include_skill: true };
    let p = plan(&host, &req).expect("plan qoder");
    assert!(!p.blocked, "{:?}", p.actions);
    assert_eq!(p.skills.len(), 1);
    assert_eq!(p.skills[0].dir, custom_qoder.join("skills"));

    let rep = apply(&p);
    assert_eq!(rep.status, "configured");
    assert!(custom_qoder.join("skills").join("craft-anything").join("SKILL.md").is_file());
    assert!(!home.join(".qoder").exists(), "default ~/.qoder must not be created when QODER_CONFIG_DIR is set");

    let doc = doctor(&host, &DoctorRequest { clients: vec!["qoder".into()], cli_path: Some(cli), extra: Vec::new(), self_test: false, engines: false });
    assert!(doc.healthy, "{:?}", doc.checks);
    let skill_check_name = format!("skill:{}", custom_qoder.join("skills").display());
    assert!(
        doc.checks.iter().any(|c| c.name == skill_check_name && c.status == CheckStatus::Ok),
        "expected Ok skill check for {skill_check_name}, got: {:?}",
        doc.checks
    );

    let un = uninstall(&host);
    assert_eq!(un.status, "removed");
    assert!(!custom_qoder.join("skills").join("craft-anything").exists());
}

/// On Windows (`Os::Windows`), `Host::var` expands leading `~\` backslash home prefixes (including
/// nested segments and bare `~\`) relative to `host.home` instead of creating a literal `~` folder.
#[test]
fn windows_host_expands_backslash_tilde_prefixes_without_literal_tilde_segment() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("Users").join("dev");
    let mut env = BTreeMap::new();
    env.insert("CODEX_HOME".into(), r"~\.codex-custom".into());
    env.insert("HERMES_HOME".into(), r"~\AppData\Local\hermes-custom".into());
    env.insert("GROK_HOME".into(), r"~/AppData\Local\grok-mixed".into());
    env.insert("KIMI_CODE_HOME".into(), r"~\sub1/sub2\kimi".into());
    env.insert("QWEN_HOME".into(), r"~\".into());
    env.insert("GEMINI_CLI_HOME".into(), "~//gem-double".into());
    env.insert("ANTIGRAVITY_HOME".into(), "~///server/share".into());
    env.insert("QODER_CONFIG_DIR".into(), r"~\.qoder-win".into());
    env.insert("COPILOT_HOME".into(), r"~/D:\copilot-abs".into());
    env.insert("PI_CODING_AGENT_DIR".into(), r"~\sub\C:\other".into());
    env.insert("CLAUDE_CONFIG_DIR".into(), "~/C:rel".into());
    env.insert("DSH_HOME".into(), r"~/\sub:name".into());
    let host = Host { home: home.clone(), env, os: Os::Windows };

    let cs = clients(&host);
    let of = |id: &str| cs.iter().find(|c| c.id == id).unwrap_or_else(|| panic!("missing {id}"));

    assert_eq!(of("codex").config, home.join(".codex-custom").join("config.toml"));
    assert_eq!(of("hermes").config, home.join("AppData").join("Local").join("hermes-custom").join("config.yaml"));
    assert_eq!(of("grok_build").config, home.join("AppData").join("Local").join("grok-mixed").join("config.toml"));
    assert_eq!(of("kimi_code").config, home.join("sub1").join("sub2").join("kimi").join("mcp.json"));
    assert_eq!(of("qwen_code").config, home.join("settings.json"));
    assert_eq!(of("qwen_code").markers[0].as_os_str(), home.as_os_str());
    assert_eq!(of("gemini_cli").config, home.join("gem-double").join(".gemini").join("settings.json"));
    assert_eq!(of("antigravity").config, home.join("server").join("share").join(".gemini").join("config").join("mcp_config.json"));
    assert_eq!(of("qoder").config, home.join(".qoder-win").join("settings.json"));
    assert_eq!(of("qoder").skills_dir, home.join(".qoder-win").join("skills"));
    assert_eq!(of("copilot_cli").config, Path::new(r"D:\copilot-abs").join("mcp-config.json"));
    #[cfg(windows)]
    assert!(of("copilot_cli").config.is_absolute());

    for id in ["codex", "hermes", "grok_build", "kimi_code", "qwen_code", "gemini_cli", "antigravity", "qoder", "pi", "claude_code", "dsh"] {
        let cfg = &of(id).config;
        assert!(cfg.is_absolute(), "{id} must stay absolute under home: {}", cfg.display());
        assert!(!cfg.components().any(|c| c.as_os_str() == Path::new("~").as_os_str()), "{id} retained a literal `~` path component: {}", cfg.display());
    }
}

/// Non-Windows hosts expand `~` and `~/...` as usual while leaving `~user/...` and backslash
/// `~\...` strings untouched as ordinary relative path segments.
#[test]
fn unix_hosts_keep_forward_tilde_and_leave_other_tilde_forms_relative() {
    for os in [Os::Linux, Os::Macos] {
        let mut env = BTreeMap::new();
        env.insert("CODEX_HOME".into(), "~/codex-unix".into());
        env.insert("HERMES_HOME".into(), "~//hermes-double".into());
        env.insert("QWEN_HOME".into(), "~/".into());
        env.insert("GROK_HOME".into(), "~other/grok".into());
        env.insert("KIMI_CODE_HOME".into(), r"~\literal-backslash".into());
        let home = Path::new("/home/u").to_path_buf();
        let host = Host { home: home.clone(), env, os };

        let cs = clients(&host);
        let of = |id: &str| cs.iter().find(|c| c.id == id).unwrap();
        assert_eq!(of("codex").config, home.join("codex-unix").join("config.toml"));
        assert_eq!(of("hermes").config, home.join("hermes-double").join("config.yaml"));
        assert_eq!(of("qwen_code").config, home.join("settings.json"));
        assert_eq!(of("qwen_code").markers[0].as_os_str(), home.as_os_str());
        assert_eq!(of("grok_build").config, home.join("~other/grok").join("config.toml"));
        assert_eq!(of("kimi_code").config, home.join(r"~\literal-backslash").join("mcp.json"));
        assert!(of("kimi_code").config.to_string_lossy().contains('~'), "Unix host must not strip `~\\`: {}", of("kimi_code").config.display());
    }
}
