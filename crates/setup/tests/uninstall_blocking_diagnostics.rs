//! Regressions for `craft_anything_setup::uninstall` blocking diagnostics (and invalid-UTF-8 vs
//! I/O remediation in `plan` and `uninstall`).

use craft_anything_setup::{ActionKind, CheckStatus, DoctorRequest, Host, Os, PlanRequest, apply, clients, config, doctor, is_owned_entry, plan, uninstall};
use serde_json::json;

/// Create an isolated Linux-layout host in `dir` with a dummy `craft-anything-cli` binary.
fn test_host(dir: &std::path::Path) -> (Host, Vec<String>) {
    let mut host = Host::with_home(dir);
    host.os = Os::Linux;
    let cli = dir.join("bin").join("craft-anything-cli");
    std::fs::create_dir_all(cli.parent().expect("bin parent")).expect("create bin dir");
    std::fs::write(&cli, "#!/bin/sh\n").expect("write dummy cli");
    (host, vec![cli.display().to_string(), "mcp".into()])
}

/// `text` with `path` replaced, so wording checks can't match words in the temp folder's path.
fn words(text: Option<&str>, path: &std::path::Path) -> String {
    text.unwrap_or_default().replace(&path.display().to_string(), "<path>")
}

/// Multi-line YAML flow syntax on our entry is readable via `YamlLoader` (`is_owned_entry == true`)
/// yet refused by `yaml::try_remove`. `uninstall` must report an `ActionKind::Invalid` removal
/// diagnostic mentioning `uninstall` (not `"Can't add our entry"` or `"Add it by hand ... setup"`)
/// while leaving all configs, siblings, and skills untouched.
#[test]
fn uninstall_reports_remove_refusal_instead_of_add_entry_failure_on_multiline_flow_yaml() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (mut host, cmd) = test_host(dir.path());
    let hermes_dir = dir.path().join("hermes-home");
    std::fs::create_dir_all(&hermes_dir).expect("hermes dir");
    host.env.insert("HERMES_HOME".into(), hermes_dir.display().to_string());

    let cursor_cfg = dir.path().join(".cursor").join("mcp.json");
    std::fs::create_dir_all(cursor_cfg.parent().expect("cursor dir")).expect("create cursor dir");
    std::fs::write(&cursor_cfg, "{\"mcpServers\": {\"other\": {\"command\": \"npx\", \"args\": [\"keep\"]}}}\n").expect("write cursor");
    let p_cursor =
        plan(&host, &PlanRequest { clients: vec!["cursor".into()], all_detected: false, command: cmd.clone(), include_skill: true }).expect("plan cursor");
    assert_eq!(apply(&p_cursor).status, "configured");
    let cursor_before = std::fs::read_to_string(&cursor_cfg).expect("read cursor before");
    let shared_skill = host.shared_skills().join("craft-anything/SKILL.md");
    assert!(shared_skill.is_file());

    let hermes_cfg = hermes_dir.join("config.yaml");
    let flow_yaml = format!(
        "model: big\nmcp_servers:\n  craft-anything: {{\n    command: {},\n    args: [\"mcp\"],\n    enabled: true\n  }}\n  other: {{command: npx, args: [\"a\"]}}\n",
        json!(&cmd[0])
    );
    std::fs::write(&hermes_cfg, &flow_yaml).expect("write flow yaml");

    let u = uninstall(&host);
    assert_eq!(u.status, "blocked");
    assert!(u.removed_entries.is_empty());
    assert!(u.removed_skill_files.is_empty());
    assert_eq!(u.blocking.len(), 1);

    let b = &u.blocking[0];
    assert_eq!(b.client, "hermes");
    assert_eq!(b.action, ActionKind::Invalid);
    assert!(b.preview.is_empty(), "blocking uninstall action must not carry a setup diff preview: {b:?}");
    let reason = words(b.reason.as_deref(), &hermes_cfg);
    let remediation = words(b.remediation.as_deref(), &hermes_cfg);
    assert!(reason.starts_with("Can't remove our entry from <path>"), "unexpected reason: {reason}");
    assert!(!reason.contains("Can't add"), "uninstall reason talked about adding: {reason}");
    assert!(remediation.contains("by hand") && remediation.contains("uninstall"), "unexpected remediation: {remediation}");
    assert!(!remediation.contains("Add it by hand") && !remediation.contains("setup"), "uninstall remediation talked about setup: {remediation}");

    assert_eq!(std::fs::read_to_string(&hermes_cfg).expect("hermes after"), flow_yaml);
    assert_eq!(std::fs::read_to_string(&cursor_cfg).expect("cursor after"), cursor_before);
    assert!(shared_skill.is_file(), "skills must remain untouched when uninstall is blocked");
}

/// An escaped YAML key (`"\x63raft-anything":`) parses as `"craft-anything"` via `YamlLoader`
/// (`is_owned_entry == true`) while `yaml::try_remove` returns `Ok(None)`. `uninstall` must emit
/// `ActionKind::Invalid` with a removal reason and remediation rather than calling
/// `inspect_client(c, &[])` (which tried to append a duplicate `craft-anything` entry and reported
/// `"Can't add our entry ... duplicated key"`).
#[test]
fn uninstall_blocks_with_invalid_action_when_escaped_yaml_key_cannot_be_removed_in_place() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (mut host, cmd) = test_host(dir.path());
    let hermes_dir = dir.path().join("hermes-home");
    std::fs::create_dir_all(&hermes_dir).expect("hermes dir");
    host.env.insert("HERMES_HOME".into(), hermes_dir.display().to_string());

    let hermes_cfg = hermes_dir.join("config.yaml");
    let escaped_yaml = format!(
        "mcp_servers:\n  \"\\x63raft-anything\":\n    command: {}\n    args:\n      - \"mcp\"\n    enabled: true\n  other:\n    command: npx\n",
        json!(&cmd[0])
    );
    std::fs::write(&hermes_cfg, &escaped_yaml).expect("write escaped yaml");

    let hermes_client = clients(&host).into_iter().find(|c| c.id == "hermes").expect("hermes client");
    let parsed = config::read_entry(&hermes_client, &escaped_yaml).expect("parse escaped yaml").expect("entry present");
    assert!(is_owned_entry(&parsed), "escaped key decodes to owned craft-anything entry");
    assert_eq!(config::remove_entry(&hermes_client, &escaped_yaml).expect("try_remove"), None);

    let u = uninstall(&host);
    assert_eq!(u.status, "blocked");
    assert_eq!(u.blocking.len(), 1);
    let b = &u.blocking[0];
    assert_eq!(b.action, ActionKind::Invalid, "blocking entry must be ActionKind::Invalid, not Update: {b:?}");
    assert!(b.preview.is_empty(), "must not include setup diff preview: {b:?}");
    let reason = words(b.reason.as_deref(), &hermes_cfg);
    let remediation = words(b.remediation.as_deref(), &hermes_cfg);
    assert!(reason.starts_with("Can't remove our entry from <path>"), "unexpected reason: {reason}");
    assert!(remediation.contains("uninstall") && !remediation.contains("setup"), "unexpected remediation: {remediation}");
    assert_eq!(std::fs::read_to_string(&hermes_cfg).expect("hermes unchanged"), escaped_yaml);
}

/// Invalid UTF-8 bytes, OS I/O errors (such as a directory at the config path), and malformed
/// config syntax must each produce accurate `reason` (keeping `Can't read <path>: ...`) and
/// `remediation` (UTF-8 vs permissions, and `uninstall` rather than `setup` during `uninstall`).
#[test]
fn uninstall_and_plan_distinguish_invalid_utf8_io_errors_and_malformed_syntax() {
    // Subcase A: invalid UTF-8 bytes in `.cursor/mcp.json`.
    let dir_utf8 = tempfile::tempdir().expect("tempdir utf8");
    let (host_utf8, cmd_utf8) = test_host(dir_utf8.path());
    let cursor_utf8 = dir_utf8.path().join(".cursor").join("mcp.json");
    std::fs::create_dir_all(cursor_utf8.parent().expect("cursor dir")).expect("mkdir");
    std::fs::write(&cursor_utf8, [0xffu8, 0xfe, 0x00]).expect("write invalid utf8");

    let p_utf8 =
        plan(&host_utf8, &PlanRequest { clients: vec!["cursor".into()], all_detected: false, command: cmd_utf8, include_skill: false }).expect("plan utf8");
    assert!(p_utf8.blocked);
    let pa = &p_utf8.actions[0];
    assert_eq!(pa.action, ActionKind::Invalid);
    assert_eq!(words(pa.reason.as_deref(), &cursor_utf8), "Can't read <path>: the file is not valid UTF-8");
    let fix = words(pa.remediation.as_deref(), &cursor_utf8);
    assert!(fix.contains("UTF-8") && fix.contains("where it is") && !fix.contains("permissions"), "{fix}");

    let d_utf8 = doctor(&host_utf8, &DoctorRequest { clients: vec!["cursor".into()], cli_path: None, extra: Vec::new(), self_test: false, engines: false });
    let dc_utf8 = d_utf8.checks.iter().find(|c| c.name == "registration:cursor").expect("doctor cursor utf8 check");
    assert_eq!(dc_utf8.status, CheckStatus::Fail);
    assert_eq!(words(Some(&dc_utf8.message), &cursor_utf8), "Can't read <path>: the file is not valid UTF-8");
    let fix = words(dc_utf8.remediation.as_deref(), &cursor_utf8);
    assert!(fix.contains("UTF-8") && !fix.contains("permissions"), "{fix}");

    let u_utf8 = uninstall(&host_utf8);
    assert_eq!(u_utf8.status, "blocked");
    let ua = &u_utf8.blocking[0];
    assert_eq!(ua.action, ActionKind::Invalid);
    assert_eq!(words(ua.reason.as_deref(), &cursor_utf8), "Can't read <path>: the file is not valid UTF-8");
    assert_eq!(words(ua.remediation.as_deref(), &cursor_utf8), "Back up <path> and save it as UTF-8 where it is, then run uninstall again.");

    // Subcase B: a folder at the config path. Windows reports reading it as access denied, so the
    // advice must still name the folder rather than permissions.
    let dir_io = tempfile::tempdir().expect("tempdir io");
    let (host_io, cmd_io) = test_host(dir_io.path());
    let cursor_dir = dir_io.path().join(".cursor").join("mcp.json");
    std::fs::create_dir_all(&cursor_dir).expect("create directory at config path");

    let p_io = plan(&host_io, &PlanRequest { clients: vec!["cursor".into()], all_detected: false, command: cmd_io, include_skill: false }).expect("plan io");
    assert!(p_io.blocked);
    assert_eq!(words(p_io.actions[0].reason.as_deref(), &cursor_dir), "Can't read <path>: it is a folder, not a config file");
    let fix = words(p_io.actions[0].remediation.as_deref(), &cursor_dir);
    assert!(fix.contains("not a folder") && fix.contains("setup") && !fix.contains("permissions"), "{fix}");

    let d_io = doctor(&host_io, &DoctorRequest { clients: vec!["cursor".into()], cli_path: None, extra: Vec::new(), self_test: false, engines: false });
    let dc_io = d_io.checks.iter().find(|c| c.name == "registration:cursor").expect("doctor cursor io check");
    assert_eq!(dc_io.status, CheckStatus::Fail);
    assert_eq!(words(Some(&dc_io.message), &cursor_dir), "Can't read <path>: it is a folder, not a config file");
    assert!(!words(dc_io.remediation.as_deref(), &cursor_dir).contains("permissions"), "{dc_io:?}");

    let u_io = uninstall(&host_io);
    assert_eq!(u_io.status, "blocked");
    let uio = &u_io.blocking[0];
    assert_eq!(uio.action, ActionKind::Invalid);
    assert_eq!(
        words(uio.remediation.as_deref(), &cursor_dir),
        "<path> must be a config file, not a folder. Move the folder elsewhere, then run uninstall again."
    );

    // Subcase C: malformed StrictJson syntax in `.pi/agent/mcp.json`.
    let dir_syn = tempfile::tempdir().expect("tempdir syn");
    let (host_syn, _) = test_host(dir_syn.path());
    let pi_cfg = dir_syn.path().join(".pi").join("agent").join("mcp.json");
    std::fs::create_dir_all(pi_cfg.parent().expect("pi dir")).expect("mkdir pi");
    std::fs::write(&pi_cfg, "{ // comments rejected in StrictJson\n}\n").expect("write malformed pi");

    let u_syn = uninstall(&host_syn);
    assert_eq!(u_syn.status, "blocked");
    let usyn = &u_syn.blocking[0];
    assert_eq!(usyn.action, ActionKind::Invalid);
    assert!(words(usyn.reason.as_deref(), &pi_cfg).starts_with("<path> can't be read as StrictJson"), "{usyn:?}");
    let fix = words(usyn.remediation.as_deref(), &pi_cfg);
    assert!(fix.contains("where it is") && fix.contains("uninstall") && !fix.contains("setup") && !fix.contains("move"), "{fix}");
}

/// Ordinary valid JSONC and YAML configs (including a one-line flow sibling) continue to uninstall
/// cleanly while preserving every unrelated sibling entry.
#[test]
fn valid_configs_with_siblings_still_uninstall_cleanly() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (mut host, cmd) = test_host(dir.path());
    let hermes_dir = dir.path().join("hermes-home");
    std::fs::create_dir_all(&hermes_dir).expect("hermes dir");
    host.env.insert("HERMES_HOME".into(), hermes_dir.display().to_string());

    let cursor_cfg = dir.path().join(".cursor").join("mcp.json");
    std::fs::create_dir_all(cursor_cfg.parent().expect("cursor dir")).expect("mkdir cursor");
    let cursor_init = "{\n  // keep comment\n  \"mcpServers\": {\n    \"other\": {\"command\": \"npx\", \"args\": [\"x\"]}\n  }\n}\n";
    std::fs::write(&cursor_cfg, cursor_init).expect("write cursor");

    let hermes_cfg = hermes_dir.join("config.yaml");
    let hermes_init = "model: fast\nmcp_servers:\n  other: {command: npx, args: [\"a\"]}\n";
    std::fs::write(&hermes_cfg, hermes_init).expect("write hermes");

    let p =
        plan(&host, &PlanRequest { clients: vec!["cursor".into(), "hermes".into()], all_detected: false, command: cmd, include_skill: true }).expect("plan");
    assert_eq!(apply(&p).status, "configured");

    let u = uninstall(&host);
    assert_eq!(u.status, "removed", "{u:?}");
    assert!(u.blocking.is_empty());
    assert_eq!(u.removed_entries.len(), 2);
    assert_eq!(std::fs::read_to_string(&hermes_cfg).expect("hermes after"), hermes_init);
    let cursor_after = std::fs::read_to_string(&cursor_cfg).expect("cursor after");
    assert!(cursor_after.contains("// keep comment") && cursor_after.contains("\"other\"") && !cursor_after.contains("craft-anything"));
}
