//! Reading and editing one client's config text, per format. Pure text in, text out.

use crate::clients::{Client, Format, SERVER_NAME, Shape, is_owned_entry};
use crate::yaml;
use craft_anything_formats::jsonc;
use serde_json::Value;

/// Parse the whole config to JSON-like data. An empty file is `{}` unless the client forbids it.
pub fn parse(client: &Client, text: &str) -> Result<Value, String> {
    if text.trim().is_empty() && client.empty_is_invalid {
        return Err("the file is empty, and this app requires valid JSON".into());
    }
    match client.format {
        Format::Jsonc => jsonc::parse(text, false).map(|d| d.to_value()),
        Format::StrictJson => {
            if text.trim().is_empty() {
                return Ok(Value::Object(Default::default()));
            }
            jsonc::parse(text, true).map(|d| d.to_value())
        }
        Format::Toml => {
            let t: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
            serde_json::to_value(t).map_err(|e| e.to_string())
        }
        Format::Yaml => yaml::parse(text),
        Format::Manual => Err("this app's config is not edited automatically".into()),
    }
}

/// Our entry, if present.
pub fn read_entry(client: &Client, text: &str) -> Result<Option<Value>, String> {
    let v = parse(client, text)?;
    let mut cur = &v;
    for k in client.entry_path() {
        match cur.get(k) {
            Some(n) => cur = n,
            None => return Ok(None),
        }
    }
    Ok(Some(cur.clone()))
}

/// True when the client lists our server as disabled (Grok Build's `disabled_mcp_servers`).
pub fn is_disabled(client: &Client, text: &str) -> bool {
    if client.id != "grok_build" {
        return false;
    }
    parse(client, text).ok().and_then(|v| v.get("disabled_mcp_servers").and_then(|d| d.as_array()).map(|a| a.iter().any(|x| x == SERVER_NAME))).unwrap_or(false)
}

/// True when the person turned our entry off themselves (Codex's `enabled = false`). Setup leaves
/// that as it is; the doctor says so.
pub fn entry_turned_off(client: &Client, text: &str) -> bool {
    client.shape == Shape::Codex && read_entry(client, text).ok().flatten().is_some_and(|e| e.get("enabled") == Some(&Value::Bool(false)))
}

/// Text with our entry set to `entry`.
pub fn set_entry(client: &Client, text: &str, entry: &Value) -> Result<String, String> {
    match client.format {
        Format::Jsonc | Format::StrictJson => {
            parse(client, text)?;
            let doc = jsonc::parse(text, client.format == Format::StrictJson)?;
            doc.set(&client.entry_path(), entry)
        }
        Format::Toml => toml_set(client, text, entry),
        Format::Yaml => {
            let table = client.table.first().copied().unwrap_or("mcp_servers");
            yaml::set(text, table, SERVER_NAME, entry)
        }
        Format::Manual => Err("this app's config is not edited automatically".into()),
    }
}

/// Text with our entry removed, or `None` when it wasn't there.
pub fn remove_entry(client: &Client, text: &str) -> Result<Option<String>, String> {
    match client.format {
        Format::Jsonc | Format::StrictJson => {
            let doc = jsonc::parse(text, client.format == Format::StrictJson)?;
            Ok(doc.remove(&client.entry_path()))
        }
        Format::Toml => {
            let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
            let table = client.table.first().copied().unwrap_or("mcp_servers");
            let removed = doc.get_mut(table).and_then(|t| t.as_table_like_mut()).and_then(|t| t.remove(SERVER_NAME)).is_some();
            Ok(removed.then(|| with_line_endings_of(text, doc.to_string())))
        }
        Format::Yaml => {
            let table = client.table.first().copied().unwrap_or("mcp_servers");
            yaml::try_remove(text, table, SERVER_NAME)
        }
        Format::Manual => Ok(None),
    }
}

/// `edited` with Windows line endings when every line of `original` had them. toml_edit writes its
/// own line breaks as `\n`; a CRLF file keeps its endings, as the JSONC and YAML edits do.
///
/// A file with any bare `\n` is left as toml_edit wrote it: a line break inside a multi-line string
/// is part of the value. The swap is also dropped if it would change a value we wrote.
fn with_line_endings_of(original: &str, edited: String) -> String {
    let crlf = original.matches("\r\n").count();
    if crlf == 0 || crlf != original.matches('\n').count() {
        return edited;
    }
    let converted = edited.replace("\r\n", "\n").replace('\n', "\r\n");
    let read = |t: &str| toml::from_str::<toml::Table>(t).ok();
    if read(&converted) == read(&edited) { converted } else { edited }
}

fn json_to_toml(v: &Value) -> Option<toml_edit::Value> {
    Some(match v {
        Value::String(s) => s.as_str().into(),
        Value::Bool(b) => (*b).into(),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i.into(),
            None => n.as_f64()?.into(),
        },
        Value::Array(items) => {
            let mut a = toml_edit::Array::new();
            for it in items {
                a.push(json_to_toml(it)?);
            }
            toml_edit::Value::Array(a)
        }
        Value::Object(m) => {
            let mut t = toml_edit::InlineTable::new();
            for (k, v) in m {
                t.insert(k, json_to_toml(v)?);
            }
            toml_edit::Value::InlineTable(t)
        }
        Value::Null => return None,
    })
}

/// `want` set into `target` where it stands: a setting that already has the value is left alone, a
/// different one is replaced keeping its comment, and a table of ours is merged into the person's
/// (regular or inline) instead of replacing it, so their other settings and comments stay.
fn merge_in_place(target: &mut dyn toml_edit::TableLike, want: &serde_json::Map<String, Value>) -> Result<(), String> {
    for (k, v) in want {
        if let Value::Object(inner) = v
            && let Some(child) = target.get_mut(k).and_then(|i| i.as_table_like_mut())
        {
            merge_in_place(child, inner)?;
            continue;
        }
        let mut new = json_to_toml(v).ok_or_else(|| format!("cannot write {k} as TOML"))?;
        if let Some(have) = target.get(k).and_then(|i| i.as_value()) {
            if have.to_string().trim() == new.to_string().trim() {
                continue;
            }
            *new.decor_mut() = have.decor().clone();
        }
        // Through the entry, so the key stays as written (a comment on the line above it is part of
        // the key); `insert` would put in a fresh key.
        match target.entry(k) {
            toml_edit::Entry::Occupied(mut entry) => {
                entry.insert(toml_edit::Item::Value(new));
            }
            toml_edit::Entry::Vacant(entry) => {
                entry.insert(toml_edit::Item::Value(new));
            }
        }
    }
    Ok(())
}

fn toml_set(client: &Client, text: &str, entry: &Value) -> Result<String, String> {
    let old = read_entry(client, text)?;
    let mut doc: toml_edit::DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| e.to_string())?;
    let table_name = client.table.first().copied().unwrap_or("mcp_servers");
    let Value::Object(fields) = entry else { return Err("entry must be a table".into()) };
    if doc.get(table_name).is_none() {
        let mut t = toml_edit::Table::new();
        t.set_implicit(true);
        doc.insert(table_name, toml_edit::Item::Table(t));
    }
    let inline_parent = doc.get(table_name).is_some_and(toml_edit::Item::is_inline_table);
    let servers = doc.get_mut(table_name).and_then(|t| t.as_table_like_mut()).ok_or_else(|| format!("`{table_name}` exists but is not a table"))?;
    // Only our own entry is merged into (a foreign one, such as an HTTP `url` Codex would refuse
    // beside a `command`, is replaced, as the plan says it will be). The person's other settings
    // and comments stay, whether the entry or its parent table is written inline.
    let merge = client.shape == Shape::Codex && old.as_ref().is_some_and(is_owned_entry);
    if merge && let Some(existing) = servers.get_mut(SERVER_NAME).and_then(|i| i.as_table_like_mut()) {
        merge_in_place(existing, fields)?;
    } else if inline_parent {
        let inline = json_to_toml(entry).ok_or_else(|| "cannot write entry as TOML".to_string())?;
        servers.insert(SERVER_NAME, toml_edit::Item::Value(inline));
    } else {
        let mut ours = toml_edit::Table::new();
        for (k, v) in fields {
            let tv = json_to_toml(v).ok_or_else(|| format!("cannot write {k} as TOML"))?;
            ours.insert(k, toml_edit::Item::Value(tv));
        }
        match servers.get_mut(SERVER_NAME).and_then(|i| i.as_table_mut()) {
            Some(existing) => {
                // Keep the existing table's position and decor; replace its keys.
                existing.clear();
                for (k, v) in ours.iter() {
                    existing.insert(k, v.clone());
                }
            }
            None => {
                servers.insert(SERVER_NAME, toml_edit::Item::Table(ours));
            }
        }
    }
    if client.id == "grok_build"
        && let Some(arr) = doc.get_mut("disabled_mcp_servers").and_then(|d| d.as_array_mut())
    {
        arr.retain(|v| v.as_str() != Some(SERVER_NAME));
    }
    Ok(with_line_endings_of(text, doc.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clients::{Host, clients};
    use serde_json::json;

    fn client(id: &str) -> Client {
        clients(&Host::with_home("/h")).into_iter().find(|c| c.id == id).unwrap()
    }

    #[test]
    fn toml_merge_preserves_comments() {
        let c = client("codex");
        let text = "# Codex settings\nmodel = \"x\" # pick one\n\n[mcp_servers.other]\ncommand = \"o\" # theirs\n";
        let e = c.desired_entry(&["/b/craft-anything-cli".into(), "mcp".into()]);
        let new = set_entry(&c, text, &e).unwrap();
        assert!(new.contains("# Codex settings") && new.contains("# pick one") && new.contains("# theirs"));
        assert_eq!(read_entry(&c, &new).unwrap(), Some(e.clone()));
        assert_eq!(parse(&c, &new).unwrap()["mcp_servers"]["other"]["command"], "o");
        let again = set_entry(&c, &new, &e).unwrap();
        assert_eq!(again, new);
        let removed = remove_entry(&c, &new).unwrap().unwrap();
        assert_eq!(read_entry(&c, &removed).unwrap(), None);
        assert!(removed.contains("# theirs"));
        assert!(parse(&c, "a = [").is_err());
    }

    /// #540: moving an older Codex entry to the approving one keeps what the person added to it.
    #[test]
    fn a_codex_entry_keeps_the_persons_own_settings_when_it_gains_the_approval() {
        let c = client("codex");
        let text = "[mcp_servers.craft-anything]\ncommand = \"/old/craft-anything-cli\"\nargs = [\"mcp\"]\nenabled_tools = [\"core_a\"] # mine\ntools = { other = { approval_mode = \"prompt\" } }\n\n[mcp_servers.craft-anything.env]\nK = \"v\"\n";
        let e = c.desired_entry(&["/b/craft-anything-cli".into(), "mcp".into()]);
        assert!(!c.entry_is_current(&read_entry(&c, text).unwrap().unwrap(), &e));
        let new = set_entry(&c, text, &e).unwrap();
        let got = read_entry(&c, &new).unwrap().unwrap();
        assert_eq!(got["command"], "/b/craft-anything-cli");
        assert_eq!(got["enabled_tools"][0], "core_a");
        assert_eq!(got["env"]["K"], "v");
        assert_eq!(got["tools"]["other"]["approval_mode"], "prompt", "their other approvals stay");
        assert_eq!(got["tools"]["rebuild_start"]["approval_mode"], "approve");
        assert!(got.get("default_tools_approval_mode").is_none());
        assert!(new.contains("# mine"));
        assert!(c.entry_is_current(&got, &e), "their extra settings don't make it out of date");
        assert_eq!(set_entry(&c, &new, &e).unwrap(), new, "a second run changes nothing");
    }

    /// #540: a foreign entry (here an HTTP one) is replaced whole, not merged into, and an entry the
    /// person turned off is reported as such.
    #[test]
    fn a_foreign_codex_entry_is_replaced_and_a_disabled_one_is_noticed() {
        let c = client("codex");
        let e = c.desired_entry(&["/b/craft-anything-cli".into(), "mcp".into()]);
        let foreign = "[mcp_servers.craft-anything]\nurl = \"https://example.test/mcp\"\n";
        let got = read_entry(&c, &set_entry(&c, foreign, &e).unwrap()).unwrap().unwrap();
        assert!(got.get("url").is_none(), "{got}");
        assert_eq!(got, e);
        let off = set_entry(&c, "", &e).unwrap().replace("startup_timeout_sec", "enabled = false\nstartup_timeout_sec");
        assert!(entry_turned_off(&c, &off) && !entry_turned_off(&c, &set_entry(&c, "", &e).unwrap()));
        assert!(c.entry_is_current(&read_entry(&c, &off).unwrap().unwrap(), &e), "setup keeps their choice");
    }

    /// #540: our own entry written inline keeps the person's settings too.
    #[test]
    fn an_inline_codex_entry_keeps_the_persons_own_settings() {
        let c = client("codex");
        let e = c.desired_entry(&["/b/craft-anything-cli".into(), "mcp".into()]);
        let text = "[mcp_servers]\ncraft-anything = { command = \"/old/craft-anything-cli\", args = [\"mcp\"], enabled = false, disabled_tools = [\"x\"] }\n";
        let new = set_entry(&c, text, &e).unwrap();
        let got = read_entry(&c, &new).unwrap().unwrap();
        assert_eq!(got["command"], "/b/craft-anything-cli");
        assert_eq!(got["enabled"], false);
        assert_eq!(got["disabled_tools"][0], "x");
        assert_eq!(got["tools"]["rebuild_start"]["approval_mode"], "approve");
        assert_eq!(set_entry(&c, &new, &e).unwrap(), new, "a second run changes nothing");
    }

    /// #540: comments on an inline entry, in its fields and in a `tools` table stay.
    #[test]
    fn comments_on_our_codex_entry_and_its_tools_table_stay() {
        let c = client("codex");
        let e = c.desired_entry(&["/b/craft-anything-cli".into(), "mcp".into()]);
        let inline = "[mcp_servers]\ncraft-anything = { command = \"/old/craft-anything-cli\", args = [\"mcp\"], enabled = false } # mine\n";
        let new = set_entry(&c, inline, &e).unwrap();
        assert!(new.contains("# mine"), "{new}");
        let got = read_entry(&c, &new).unwrap().unwrap();
        assert_eq!((got["enabled"].clone(), got["command"].clone()), (Value::Bool(false), Value::from("/b/craft-anything-cli")));
        assert_eq!(got["tools"]["rebuild_start"]["approval_mode"], "approve");
        assert_eq!(set_entry(&c, &new, &e).unwrap(), new);

        let table = "[mcp_servers.craft-anything]\ncommand = \"/old/craft-anything-cli\" # the program\nargs = [\"mcp\"]\n\n# about tools\n[mcp_servers.craft-anything.tools]\n# keep me\nother = { approval_mode = \"prompt\" } # theirs\n";
        let new = set_entry(&c, table, &e).unwrap();
        for note in ["# the program", "# about tools", "# keep me", "# theirs"] {
            assert!(new.contains(note), "{note} lost:\n{new}");
        }
        let got = read_entry(&c, &new).unwrap().unwrap();
        assert_eq!(got["tools"]["other"]["approval_mode"], "prompt");
        assert_eq!(got["tools"]["rebuild_start"]["approval_mode"], "approve");
        assert_eq!(set_entry(&c, &new, &e).unwrap(), new, "a second run changes nothing");
    }

    /// #540: a comment on the line above a key stays when that key's value changes.
    #[test]
    fn a_comment_above_a_key_stays_when_its_value_changes() {
        let c = client("codex");
        let e = c.desired_entry(&["/b/craft-anything-cli".into(), "mcp".into()]);
        let text = "[mcp_servers.craft-anything]\n# the program\ncommand = \"/old/craft-anything-cli\"\nargs = [\"mcp\"]\n# how long to wait\nstartup_timeout_sec = 10\n";
        let new = set_entry(&c, text, &e).unwrap();
        assert!(new.contains("# the program\ncommand = \"/b/craft-anything-cli\""), "{new}");
        assert!(new.contains("# how long to wait\nstartup_timeout_sec = 30"), "{new}");
        assert_eq!(set_entry(&c, &new, &e).unwrap(), new, "a second run changes nothing");
    }

    #[test]
    fn grok_enables_disabled_server() {
        let c = client("grok_build");
        let text = "disabled_mcp_servers = [\"craft-anything\", \"x\"]\n";
        assert!(is_disabled(&c, text));
        let new = set_entry(&c, text, &json!({"command": "c", "args": ["mcp"]})).unwrap();
        assert!(!is_disabled(&c, &new));
        assert!(new.contains("\"x\""));
    }

    #[test]
    fn strict_and_empty_rules() {
        let pi = client("pi");
        assert!(parse(&pi, "").is_err());
        assert!(parse(&pi, "{} // c").is_err());
        let z = client("zcode");
        let new = set_entry(&z, "", &json!({"command": "c"})).unwrap();
        assert_eq!(read_entry(&z, &new).unwrap(), Some(json!({"command": "c"})));
        assert_eq!(parse(&z, &new).unwrap()["mcp"]["servers"]["craft-anything"]["command"], "c");
    }
}
