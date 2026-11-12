//! What each adapter's builder gets, as typed data: our tool server, approvals from us, a session
//! it can continue, a note during a turn, and a message of any length.
//!
//! Two of those the adapter already answers for itself — [`AgentAdapter::resumes`], and
//! [`AgentAdapter::prompt_on_stdin`] (or ACP, which carries the message in a session rather than in
//! argv). The other three are spread over an adapter's arguments, its environment or its ACP
//! session, so they are declared here, one row per adapter, and tests check every declaration
//! against what the adapter hands a builder. An adapter without a row fails them, which is the
//! point: what a builder gets is stated in one place, and a new agent can't quietly get less.

use super::adapters::Plain;
use super::{AgentAdapter, AgentAdapters};

/// Whether a builder gets one thing, and when it doesn't, why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Given {
    Yes,
    No,
    /// Not there yet, with the gap in plain words (a run nobody has done, a mode not built).
    NotYet(&'static str),
}

impl Given {
    pub fn given(self) -> bool {
        self == Given::Yes
    }
}

/// What one adapter's builder gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grants {
    /// The builder can call Craft Anything's tools.
    pub tool_server: Given,
    /// A change it wants to make is decided by our approval policy (the person, or Auto mode)
    /// rather than by the agent alone.
    pub approvals: Given,
    /// It can carry on a saved session.
    pub resume: Given,
    /// A note the person adds during a build reaches the builder before its turn ends.
    pub mid_turn_notes: Given,
    /// The message doesn't go through argv, so its length isn't the command line's limit.
    pub long_message: Given,
}

/// How a run reaches the agent by default: its arguments and environment, or an ACP session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Path {
    /// A per-message CLI run: what the builder gets is in its arguments, or its environment.
    Argv,
    /// An ACP session, where the server is handed over in `session/new` and permission requests
    /// come back to us.
    Acp,
}

/// One adapter's declaration. `resume` and `long_message` aren't here: the adapter answers those.
struct Declared {
    id: &'static str,
    path: Path,
    tool_server: Given,
    approvals: Given,
    mid_turn_notes: Given,
}

/// The table, one row per built-in adapter and the plain one. An ACP row's server and approvals
/// ride the session; a per-message row's are checked against its arguments and environment.
const DECLARED: &[Declared] = &[
    Declared { id: "claude-code", path: Path::Argv, tool_server: Given::Yes, approvals: Given::Yes, mid_turn_notes: Given::Yes },
    // `codex exec` can't stop to ask anyone, so a tool is either pre-approved or refused by the
    // person's own Codex policy (#540); commands stay inside Codex's sandbox.
    Declared { id: "codex", path: Path::Argv, tool_server: Given::Yes, approvals: Given::No, mid_turn_notes: Given::No },
    // No server in its arguments and no environment variable for one: a Gemini CLI builder runs
    // without our tools. Its ACP mode would hand them over, but that isn't the default.
    Declared { id: "gemini-cli", path: Path::Argv, tool_server: Given::No, approvals: Given::No, mid_turn_notes: Given::No },
    // The server goes in a private `OPENCODE_CONFIG` file the run names (#481); `--auto` decides
    // what it may do by itself.
    Declared { id: "opencode", path: Path::Argv, tool_server: Given::Yes, approvals: Given::No, mid_turn_notes: Given::No },
    // Run with `--dangerously-skip-permissions`, and nothing hands it our server.
    Declared { id: "antigravity", path: Path::Argv, tool_server: Given::No, approvals: Given::No, mid_turn_notes: Given::No },
    Declared { id: "hermes", path: Path::Argv, tool_server: Given::No, approvals: Given::No, mid_turn_notes: Given::No },
    // Nothing can ask in `-p`, so its changing tools are switched off outside a build instead.
    Declared { id: "maki", path: Path::Argv, tool_server: Given::No, approvals: Given::No, mid_turn_notes: Given::No },
    Declared { id: "aider", path: Path::Argv, tool_server: Given::No, approvals: Given::No, mid_turn_notes: Given::No },
    Declared { id: "copilot", path: Path::Acp, tool_server: Given::Yes, approvals: Given::Yes, mid_turn_notes: Given::No },
    Declared { id: "grok", path: Path::Acp, tool_server: Given::Yes, approvals: Given::Yes, mid_turn_notes: Given::No },
    // Run one-shot it had no approval flag and no server (#21); it now always runs over ACP.
    Declared { id: "qwen-code", path: Path::Acp, tool_server: Given::Yes, approvals: Given::Yes, mid_turn_notes: Given::No },
    // Its print path gets neither; ACP (`acp = true` on the backend) hands over the server and
    // its permission requests, but isn't what a run does unless the person turned it on.
    Declared { id: "devin", path: Path::Argv, tool_server: Given::No, approvals: Given::No, mid_turn_notes: Given::No },
    // Its headless profile takes a patch overlay that holds the server, but no run has checked
    // that what it wants to do reaches our approval policy.
    Declared {
        id: "dsh",
        path: Path::Argv,
        tool_server: Given::Yes,
        approvals: Given::NotYet("its headless profile asks nothing of the host, and no run has checked whether an approval request reaches us"),
        mid_turn_notes: Given::No,
    },
    // Any other CLI: the prompt as its last argument, its output streamed as text.
    Declared {
        id: "plain",
        path: Path::Argv,
        tool_server: Given::No,
        approvals: Given::NotYet("nothing asks us; an agent with no adapter of its own is driven as a plain command line"),
        mid_turn_notes: Given::No,
    },
];

fn declared(id: &str) -> Option<&'static Declared> {
    DECLARED.iter().find(|d| d.id == id)
}

/// What `adapter`'s builder gets. `resume` and `long_message` come from the adapter itself; the
/// rest from the table, with [`Given::NotYet`] for an adapter that left the table.
pub fn grants(adapter: &dyn AgentAdapter) -> Grants {
    let id = adapter.ids().first().copied().unwrap_or("plain");
    let acp = adapter.requires_acp();
    let path = declared(id).map_or(if acp { Path::Acp } else { Path::Argv }, |d| d.path);
    let not_stated = Given::NotYet("this adapter states nothing here yet");
    Grants {
        tool_server: declared(id).map_or(not_stated, |d| d.tool_server),
        approvals: declared(id).map_or(not_stated, |d| d.approvals),
        resume: if adapter.resumes() { Given::Yes } else { Given::No },
        mid_turn_notes: declared(id).map_or(not_stated, |d| d.mid_turn_notes),
        // ACP hands the message over in the session, so no command line is involved either way.
        long_message: if adapter.prompt_on_stdin() || path == Path::Acp { Given::Yes } else { Given::No },
    }
}

/// Every adapter the app knows, with what its builder gets, by agent id. The plain adapter comes
/// last, for an agent no adapter knows.
pub fn table(adapters: &AgentAdapters) -> Vec<(&'static str, Grants)> {
    let mut rows: Vec<(&'static str, Grants)> = adapters.all().iter().map(|a| (a.ids().first().copied().unwrap_or("plain"), grants(a.as_ref()))).collect();
    rows.push(("plain", grants(&Plain)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_agents::{APPROVAL_TOOL, ArgsCx, MCP_NAME, McpCommand};
    use crate::config::BackendOptions;
    use std::path::Path as StdPath;

    fn adapters() -> Vec<std::sync::Arc<dyn AgentAdapter>> {
        AgentAdapters::built_in().all().to_vec()
    }

    /// What a builder run of `adapter` hands the process: its arguments, and the environment
    /// variable that names our server when it goes that way.
    fn builder_run(adapter: &dyn AgentAdapter) -> (Vec<String>, Option<String>) {
        let mcp = McpCommand { command: "craft-anything-cli".into(), args: vec!["mcp".into()], notes: Some(StdPath::new("notes").to_path_buf()) };
        let options = BackendOptions::default();
        let cx =
            ArgsCx { options: &options, prompt: "make a game", preamble: "", mcp: &mcp, resume: None, builder: true, reads_checked: false, question: false };
        (adapter.args(&cx), adapter.mcp_env(&mcp).ok().flatten().map(|(k, v)| format!("{k}={v}")))
    }

    /// Whether the run carries someone else's program to reach our server: in an argument (Claude
    /// Code's `--mcp-config`, Codex's `-c`, OpenCode's config file name), in the environment, or
    /// in a file an argument names (the harness's overlay).
    fn carries_server(argv: &[String], env: Option<&str>, command: &str) -> bool {
        let names = |text: &str| text.contains(MCP_NAME) || text.contains(command);
        argv.iter().any(|a| names(a)) || env.is_some_and(names) || argv.iter().any(|a| std::fs::read_to_string(a).is_ok_and(|text| names(&text)))
    }

    fn row_of(adapter: &dyn AgentAdapter) -> &'static Declared {
        let id = adapter.ids().first().copied().unwrap_or_default();
        declared(id).unwrap_or_else(|| panic!("{id} has no row: say what its builder gets"))
    }

    #[test]
    fn every_adapter_says_what_its_builder_gets() {
        for a in adapters() {
            let row = row_of(a.as_ref());
            // A row can say "not yet", but it has to say why, so nobody has to read the adapter
            // to learn what is missing.
            for (column, given) in [("tool server", row.tool_server), ("approvals", row.approvals), ("notes", row.mid_turn_notes)] {
                if let Given::NotYet(why) = given {
                    assert!(!why.trim().is_empty(), "{}: the {column} entry says nothing about what is missing", row.id);
                }
            }
        }
        assert!(declared("plain").is_some(), "the plain adapter, for an agent with none of its own, has a row");
    }

    #[test]
    fn a_tool_server_is_declared_exactly_when_a_run_carries_one() {
        for a in adapters() {
            let row = row_of(a.as_ref());
            let (argv, env) = builder_run(a.as_ref());
            match row.path {
                Path::Argv => {
                    let carried = carries_server(&argv, env.as_deref(), "craft-anything-cli");
                    assert_eq!(
                        row.tool_server.given(),
                        carried,
                        "{}: declared {:?}, but its run {}",
                        row.id,
                        row.tool_server,
                        if carried { "hands over our server" } else { "hands over none" }
                    );
                    assert!(!a.requires_acp(), "{} runs per message by default, so its row can't be ACP", row.id);
                }
                Path::Acp => {
                    assert!(a.requires_acp(), "{} is declared as ACP, so every run of it is one", row.id);
                    assert_eq!(row.tool_server, Given::Yes, "{}: an ACP session always carries our server", row.id);
                    assert_eq!(row.approvals, Given::Yes, "{}: ACP asks us before a change", row.id);
                    assert_eq!(row.mid_turn_notes, Given::No, "{}: no ACP run takes a note mid-turn", row.id);
                }
            }
        }
    }

    #[test]
    fn declared_notes_and_approvals_match_the_arguments() {
        for a in adapters() {
            let row = row_of(a.as_ref());
            let g = grants(a.as_ref());
            let (argv, _) = builder_run(a.as_ref());
            let notes = argv.iter().any(|w| w.contains("builder-notes"));
            assert_eq!(
                g.mid_turn_notes.given(),
                notes,
                "{}: declared {:?}, but its arguments {} the notes hook",
                row.id,
                g.mid_turn_notes,
                if notes { "carry" } else { "don't carry" }
            );
            if g.approvals.given() {
                let asks = argv.iter().any(|w| w.contains(APPROVAL_TOOL)) || row.path == Path::Acp;
                assert!(asks, "{}: an approval channel is declared, but its run has none", row.id);
            }
        }
    }

    #[test]
    fn resume_and_long_messages_come_from_the_adapter() {
        for a in adapters() {
            let id = a.ids().first().copied().unwrap_or_default();
            let g = grants(a.as_ref());
            let row = row_of(a.as_ref());
            assert_eq!(g.resume.given(), a.resumes(), "{id}: resume is the adapter's own answer");
            assert_eq!(g.long_message.given(), a.prompt_on_stdin() || row.path == Path::Acp, "{id}: a long message must reach it off the command line");
        }
    }

    #[test]
    fn the_table_shows_every_adapter_and_the_plain_one() {
        let rows = table(&AgentAdapters::built_in());
        let ids: Vec<&str> = rows.iter().map(|(id, _)| *id).collect();
        assert!(ids.contains(&"claude-code") && ids.contains(&"codex"), "the built-in adapters are listed");
        assert_eq!(ids.last(), Some(&"plain"), "the plain adapter comes last");
        assert_eq!(ids.len(), adapters().len() + 1, "one row per adapter, and the plain one");
        let claude = rows.iter().find(|(id, _)| *id == "claude-code").map(|(_, g)| *g).expect("Claude Code's row");
        assert_eq!(
            claude,
            Grants { tool_server: Given::Yes, approvals: Given::Yes, resume: Given::Yes, mid_turn_notes: Given::Yes, long_message: Given::Yes },
            "Claude Code gets everything today; the other rows are what to bring up to it"
        );
    }
}
