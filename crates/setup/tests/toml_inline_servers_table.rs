use craft_anything_setup::{ActionKind, CheckStatus, DoctorRequest, Host, Os, PlanRequest, apply, clients, config, doctor, plan, uninstall};
use serde_json::json;

fn cli() -> Vec<String> {
    vec!["/b/craft-anything-cli".to_string(), "mcp".to_string()]
}

/// Setup writes into an existing inline `mcp_servers` table (empty or with sibling servers), keeps
/// comments and CRLF endings, stays idempotent, and removes cleanly.
#[test]
fn inline_mcp_servers_table_accepts_our_entry() {
    let all = clients(&Host::with_home("/h"));
    for id in ["codex", "grok_build"] {
        let client = all.iter().find(|c| c.id == id).unwrap();
        let entry = client.desired_entry(&cli());

        for (base, has_other) in [
            ("model = \"x\"\nmcp_servers = {}\n", false),
            ("model = \"x\"\nmcp_servers = { other = { command = \"o\" } } # theirs\n", true),
            ("model = \"x\"\r\nmcp_servers = { other = { command = \"o\" } } # theirs\r\n", true),
        ] {
            let updated = config::set_entry(client, base, &entry).unwrap_or_else(|e| panic!("{id} on {base:?}: {e}"));
            assert_eq!(config::read_entry(client, &updated).unwrap(), Some(entry.clone()), "{id}: {updated}");
            assert_eq!(config::set_entry(client, &updated, &entry).unwrap(), updated, "{id} should be idempotent");
            if base.contains("\r\n") {
                assert_eq!(updated.matches('\n').count(), updated.matches("\r\n").count(), "{id}: {updated:?}");
            }
            if has_other {
                let parsed = config::parse(client, &updated).unwrap();
                assert_eq!(parsed["mcp_servers"]["other"]["command"], "o", "{id}: {updated}");
                assert!(updated.contains("# theirs"), "{id}: {updated}");
            }
            let removed = config::remove_entry(client, &updated).unwrap().expect("our entry can be removed");
            assert_eq!(config::read_entry(client, &removed).unwrap(), None, "{id}: {removed}");
            if has_other {
                assert_eq!(config::parse(client, &removed).unwrap()["mcp_servers"]["other"]["command"], "o");
                assert!(removed.contains("# theirs"), "{id}: {removed}");
            }
        }
    }
}

/// An owned Codex entry inside an inline `mcp_servers` table keeps the person's own flags and
/// sibling servers, while a foreign entry is replaced whole.
#[test]
fn inline_mcp_servers_table_merges_owned_and_replaces_foreign_entries() {
    let all = clients(&Host::with_home("/h"));
    let codex = all.iter().find(|c| c.id == "codex").unwrap();
    let entry = codex.desired_entry(&cli());

    let owned = "mcp_servers = { craft-anything = { command = \"/old/craft-anything-cli\", args = [\"mcp\"], enabled = false, disabled_tools = [\"x\"] }, other = { command = \"o\" } }\n";
    let merged = config::set_entry(codex, owned, &entry).unwrap();
    let got = config::read_entry(codex, &merged).unwrap().unwrap();
    assert_eq!(got["command"], "/b/craft-anything-cli");
    assert_eq!(got["enabled"], false);
    assert_eq!(got["disabled_tools"][0], "x");
    assert_eq!(got["tools"]["rebuild_start"]["approval_mode"], "approve");
    assert_eq!(config::parse(codex, &merged).unwrap()["mcp_servers"]["other"]["command"], "o");
    assert_eq!(config::set_entry(codex, &merged, &entry).unwrap(), merged);

    let foreign = "mcp_servers = { craft-anything = { url = \"https://example.test/mcp\" }, other = { command = \"o\" } }\n";
    let replaced = config::set_entry(codex, foreign, &entry).unwrap();
    let got = config::read_entry(codex, &replaced).unwrap().unwrap();
    assert!(got.get("url").is_none(), "{got}");
    assert_eq!(got, entry);
    assert_eq!(config::parse(codex, &replaced).unwrap()["mcp_servers"]["other"]["command"], "o");
}

/// Non-table `mcp_servers` values are still refused, while a foreign inline entry inside a regular
/// `[mcp_servers]` table continues to be replaced cleanly.
#[test]
fn non_table_mcp_servers_values_are_still_refused() {
    let all = clients(&Host::with_home("/h"));
    let codex = all.iter().find(|c| c.id == "codex").unwrap();
    let entry = codex.desired_entry(&cli());

    for bad in ["mcp_servers = \"nope\"\n", "mcp_servers = 42\n", "mcp_servers = []\n"] {
        let err = config::set_entry(codex, bad, &entry).unwrap_err();
        assert!(err.contains("is not a table"), "{bad}: {err}");
    }

    let regular_foreign = "[mcp_servers]\ncraft-anything = { url = \"https://example.test/mcp\" }\nother = { command = \"o\" }\n";
    let replaced = config::set_entry(codex, regular_foreign, &entry).unwrap();
    let got = config::read_entry(codex, &replaced).unwrap().unwrap();
    assert!(got.get("url").is_none(), "{got}");
    assert_eq!(got, entry);
    assert_eq!(config::parse(codex, &replaced).unwrap()["mcp_servers"]["other"]["command"], "o");
}

/// End-to-end `plan`, `apply`, `doctor`, and `uninstall` succeed when Codex uses an inline
/// `mcp_servers` table.
#[test]
fn setup_plan_apply_doctor_and_uninstall_work_with_inline_mcp_servers() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let mut host = Host::with_home(&home);
    host.os = Os::Linux;
    std::fs::create_dir_all(home.join(".codex")).unwrap();
    let initial = "# Codex config\nmcp_servers = { other = { command = \"other-server\" } } # theirs\n";
    std::fs::write(home.join(".codex/config.toml"), initial).unwrap();
    let bin = home.join("bin/craft-anything-cli");
    std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
    std::fs::write(&bin, "#!/bin/sh\n").unwrap();

    let req = PlanRequest { clients: vec!["codex".into()], all_detected: false, command: vec![bin.display().to_string(), "mcp".into()], include_skill: false };
    let p = plan(&host, &req).unwrap();
    assert!(!p.blocked, "{p:?}");
    assert_eq!(p.actions[0].action, ActionKind::Update);
    assert_eq!(apply(&p).status, "configured");
    assert_eq!(plan(&host, &req).unwrap().change_count(), 0);

    let rep = doctor(&host, &DoctorRequest { cli_path: Some(bin), self_test: false, engines: false, ..Default::default() });
    assert_eq!(rep.checks.iter().find(|c| c.name == "registration:codex").unwrap().status, CheckStatus::Ok);

    let u = uninstall(&host);
    assert_eq!(u.status, "removed", "{u:?}");
    let after = std::fs::read_to_string(home.join(".codex/config.toml")).unwrap();
    let v: serde_json::Value = config::parse(clients(&host).iter().find(|c| c.id == "codex").unwrap(), &after).unwrap();
    assert_eq!(v["mcp_servers"], json!({"other": {"command": "other-server"}}));
    assert!(after.contains("# theirs"));
}
