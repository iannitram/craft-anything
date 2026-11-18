//! Minimal, comment-preserving edits to a YAML file's top-level `<table>:` mapping.
//!
//! We only ever add, replace or remove one child block (`  craft-anything:` and the lines indented
//! under it) inside one top-level block mapping. Everything else is left byte-for-byte. Every
//! result is re-parsed with a real YAML parser before it is written.

use serde_json::{Map, Value};
use yaml_rust2::{Yaml, YamlLoader};

/// A byte-order mark some Windows editors put in front of the file. YAML allows one there; it isn't
/// part of the first key.
const BOM: &str = "\u{feff}";

/// The file's byte-order mark, if it has one, and the text after it.
fn split_bom(text: &str) -> (&'static str, &str) {
    match text.strip_prefix(BOM) {
        Some(rest) => (BOM, rest),
        None => ("", text),
    }
}

/// Parse one YAML document to JSON-like data. An empty file is an empty mapping.
pub fn parse(text: &str) -> Result<Value, String> {
    let docs = YamlLoader::load_from_str(split_bom(text).1).map_err(|e| e.to_string())?;
    if docs.len() > 1 {
        return Err("the config must contain only one YAML document".into());
    }
    match docs.into_iter().next() {
        None | Some(Yaml::Null) => Ok(Value::Object(Map::new())),
        Some(y @ Yaml::Hash(_)) => Ok(to_json(&y, 0)),
        Some(_) => Err("the top level must be a mapping".into()),
    }
}

fn to_json(y: &Yaml, depth: usize) -> Value {
    if depth > 64 {
        return Value::Null;
    }
    match y {
        Yaml::Real(s) => s.parse::<f64>().ok().and_then(serde_json::Number::from_f64).map(Value::Number).unwrap_or(Value::String(s.clone())),
        Yaml::Integer(i) => Value::from(*i),
        Yaml::String(s) => Value::String(s.clone()),
        Yaml::Boolean(b) => Value::Bool(*b),
        Yaml::Array(a) => Value::Array(a.iter().map(|v| to_json(v, depth + 1)).collect()),
        Yaml::Hash(h) => {
            let mut m = Map::new();
            for (k, v) in h {
                let key = match k {
                    Yaml::String(s) => s.clone(),
                    Yaml::Integer(i) => i.to_string(),
                    Yaml::Boolean(b) => b.to_string(),
                    Yaml::Real(r) => r.clone(),
                    _ => continue,
                };
                m.insert(key, to_json(v, depth + 1));
            }
            Value::Object(m)
        }
        _ => Value::Null,
    }
}

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || t.starts_with('#')
}

/// `key:` with optional horizontal whitespace before `:`, possibly quoted. Returns the rest after `:`.
/// Unquoted keys require horizontal whitespace or end-of-line after `:` so plain keys containing
/// embedded colons (`craft-anything:dev:`) are not mistaken for `craft-anything:`.
fn key_line<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix(name)
        && let Some(rest) = rest.trim_start_matches([' ', '\t']).strip_prefix(':')
        && (rest.is_empty() || rest.starts_with([' ', '\t']))
    {
        return Some(rest);
    }
    let dq = scalar(&Value::String(name.to_string()));
    let sq = format!("'{}'", name.replace('\'', "''"));
    for quoted in [dq, sq] {
        if let Some(rest) = t.strip_prefix(&quoted)
            && let Some(rest) = rest.trim_start_matches([' ', '\t']).strip_prefix(':')
            && (rest.is_empty() || rest.starts_with([' ', '\t']))
        {
            return Some(rest);
        }
    }
    None
}

fn only_comment(rest: &str) -> bool {
    let t = rest.trim();
    t.is_empty() || t.starts_with('#')
}

/// Render `name:` and its mapping value as block YAML at `indent` spaces. Scalars are written as
/// JSON strings/numbers, which are valid YAML.
fn render_entry(name: &str, value: &Value, indent: usize) -> String {
    let mut out = String::new();
    let pad = " ".repeat(indent);
    out.push_str(&format!("{pad}{}:\n", key(name)));
    render_map(value, indent + 2, &mut out);
    out
}

fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()),
        Value::Null => "null".into(),
        other => other.to_string(),
    }
}

/// Plain keys stay plain (`command:`); anything unusual is quoted.
fn key(k: &str) -> String {
    let plain = !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') && !k.starts_with('-');
    if plain { k.to_string() } else { scalar(&Value::String(k.to_string())) }
}

fn render_map(value: &Value, indent: usize, out: &mut String) {
    let pad = " ".repeat(indent);
    let Value::Object(m) = value else { return };
    for (k, v) in m {
        let key = key(k);
        match v {
            Value::Object(inner) if !inner.is_empty() => {
                out.push_str(&format!("{pad}{key}:\n"));
                render_map(v, indent + 2, out);
            }
            Value::Object(_) => out.push_str(&format!("{pad}{key}: {{}}\n")),
            Value::Array(items) if !items.is_empty() => {
                out.push_str(&format!("{pad}{key}:\n"));
                for it in items {
                    out.push_str(&format!("{pad}  - {}\n", scalar(it)));
                }
            }
            Value::Array(_) => out.push_str(&format!("{pad}{key}: []\n")),
            s => out.push_str(&format!("{pad}{key}: {}\n", scalar(s))),
        }
    }
}

struct Block {
    /// Line index of `table:`.
    header: usize,
    /// One past the last line belonging to the block (trailing blank/comment lines excluded).
    end: usize,
}

/// A value after `table:` that holds nothing: `{}` (with optional horizontal whitespace inside
/// the braces), or a YAML 1.2 null (`null`, `Null`, `NULL`, `~`), with an optional comment.
fn empty_inline(rest: &str) -> bool {
    let t = rest.trim();
    if let Some(inner) = t.strip_prefix('{')
        && let Some(after) = inner.trim_start_matches([' ', '\t']).strip_prefix('}')
    {
        return only_comment(after);
    }
    ["null", "Null", "NULL", "~"]
        .iter()
        .any(|null| t.strip_prefix(null).is_some_and(|after| after.is_empty() || (after.starts_with([' ', '\t']) && only_comment(after))))
}

/// Rest of a mapping entry line after the key's `:` separator, if this line starts a key: value pair.
fn mapping_value_rest(line: &str) -> Option<&str> {
    let t = line.trim_start();
    if t.is_empty() || t.starts_with('#') || t.starts_with("- ") || t == "-" {
        return None;
    }
    let after_key = if let Some(mut s) = t.strip_prefix('"') {
        let mut escaped = false;
        loop {
            let ch = s.chars().next()?;
            s = &s[ch.len_utf8()..];
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                break;
            }
        }
        s.trim_start_matches([' ', '\t'])
    } else if let Some(mut s) = t.strip_prefix('\'') {
        loop {
            let ch = s.chars().next()?;
            s = &s[ch.len_utf8()..];
            if ch == '\'' {
                if s.starts_with('\'') {
                    s = &s[1..];
                } else {
                    break;
                }
            }
        }
        s.trim_start_matches([' ', '\t'])
    } else {
        // Plain key: find the first `:` followed by horizontal whitespace or end-of-line.
        let bytes = t.as_bytes();
        let pos = bytes.iter().enumerate().find_map(
            |(idx, &b)| {
                if b == b':' && matches!(bytes.get(idx + 1), None | Some(b' ' | b'\t')) { Some(idx) } else { None }
            },
        )?;
        return t.get(pos + 1..);
    };
    after_key.strip_prefix(':').filter(|rest| rest.is_empty() || rest.starts_with([' ', '\t']))
}

/// True when `lines[..end]` parses as a complete YAML document on its own (retaining earlier
/// `&anchor` and `%TAG` context while rejecting unclosed flow collections or quotes).
fn prefix_parses(lines: &[&str], end: usize) -> bool {
    let prefix = lines.get(..end).map(|s| s.join("\n")).unwrap_or_default();
    YamlLoader::load_from_str(&prefix).is_ok()
}

/// True when the value after `:` does not finish on `line_idx` (even with earlier document
/// anchors in scope).
fn unclosed_inline_value(lines: &[&str], line_idx: usize, rest: &str) -> bool {
    let v = rest.trim_start_matches([' ', '\t']);
    !only_comment(v) && YamlLoader::load_from_str(v).is_err() && !prefix_parses(lines, line_idx + 1)
}

/// True when the remainder after `key:` carries no value on this line (at most YAML node
/// properties like `&anchor` / `!tag` and a `#` comment), so the entry's value starts on the
/// next non-blank line.
fn can_open_next_line_flow(rest: &str) -> bool {
    for tok in rest.split([' ', '\t']).filter(|s| !s.is_empty()) {
        if tok.starts_with('#') {
            break;
        }
        if !tok.starts_with(['&', '!']) {
            return false;
        }
    }
    true
}

/// True when the next non-blank, non-comment line in `from..end` opens a flow collection.
fn next_nonblank_starts_flow(lines: &[&str], from: usize, end: usize) -> bool {
    lines.get(from..end).unwrap_or(&[]).iter().find(|l| !is_blank_or_comment(l)).is_some_and(|l| l.trim_start().starts_with(['{', '[']))
}

fn flow_refusal(table: &str) -> String {
    format!("a `{table}` entry spans multiple lines with inline flow syntax (`{{...}}`/`[...]`); edit this file by hand")
}

/// The top-level `table:` block, if the file has one. An inline value is refused unless it holds
/// nothing (see [`empty_inline`]).
fn find_table(lines: &[&str], table: &str) -> Result<Option<Block>, String> {
    for (i, l) in lines.iter().enumerate() {
        if indent_of(l) != 0 || is_blank_or_comment(l) {
            continue;
        }
        if let Some(rest) = key_line(l, table) {
            if !only_comment(rest) {
                // A value that carries on onto indented lines isn't empty: `null` then `  more` is text.
                let continued = lines.iter().skip(i + 1).find(|l2| !is_blank_or_comment(l2)).is_some_and(|l2| indent_of(l2) > 0);
                if empty_inline(rest) && !continued {
                    return Ok(Some(Block { header: i, end: i + 1 }));
                }
                return Err(format!("`{table}` uses inline (flow) style; edit it by hand or convert it to a block mapping"));
            }
            let mut end = i + 1;
            for (j, l2) in lines.iter().enumerate().skip(i + 1) {
                if is_blank_or_comment(l2) {
                    continue;
                }
                if indent_of(l2) == 0 {
                    break;
                }
                end = j + 1;
            }
            return Ok(Some(Block { header: i, end }));
        }
    }
    Ok(None)
}

/// Child block `[start, end)` named `name` inside the table block, or an error when a child
/// entry spans multiple lines with inline flow syntax (`{...}` / `[...]`) whose bounds cannot be
/// inferred safely by indentation alone.
fn find_child(lines: &[&str], block: &Block, table: &str, name: &str) -> Result<Option<(usize, usize)>, String> {
    const FLOW_PUNCT: [char; 5] = ['{', '}', '[', ']', ','];
    let mut child_indent = None;
    let mut found = None;
    let mut i = block.header + 1;
    while i < block.end {
        let l = lines.get(i).copied().unwrap_or_default();
        if is_blank_or_comment(l) {
            i += 1;
            continue;
        }
        let ind = indent_of(l);
        let ci = *child_indent.get_or_insert(ind);
        if ind <= ci && l.trim_start().starts_with(FLOW_PUNCT) {
            return Err(flow_refusal(table));
        }
        if ind == ci {
            let may_open_flow = mapping_value_rest(l).is_none_or(can_open_next_line_flow);
            if !prefix_parses(lines, i) || (may_open_flow && next_nonblank_starts_flow(lines, i + 1, block.end)) {
                return Err(flow_refusal(table));
            }
            if mapping_value_rest(l).is_some_and(|rest| unclosed_inline_value(lines, i, rest)) {
                return Err(flow_refusal(table));
            }
            if found.is_none() && key_line(l, name).is_some() {
                let mut end = i + 1;
                for (j, l2) in lines.iter().enumerate().take(block.end).skip(i + 1) {
                    if is_blank_or_comment(l2) {
                        continue;
                    }
                    if indent_of(l2) <= ci {
                        if l2.trim_start().starts_with(FLOW_PUNCT) {
                            return Err(flow_refusal(table));
                        }
                        break;
                    }
                    end = j + 1;
                }
                found = Some((i, end));
            }
        }
        i += 1;
    }
    Ok(found)
}

fn child_indent(lines: &[&str], block: &Block) -> usize {
    lines.iter().take(block.end).skip(block.header + 1).find(|l| !is_blank_or_comment(l)).map(|l| indent_of(l)).unwrap_or(2)
}

/// One line of the file and the line ending it came with (`"\r\n"` or `"\n"`).
type Line = (String, &'static str);

/// Split into lines, remembering each line's own ending, so lines we don't touch come back
/// byte for byte. The last line always gets an ending (as before), the file's usual one.
/// Returns the lines and the ending to use for lines we add: CRLF when most lines have it.
fn split(text: &str) -> (Vec<Line>, &'static str) {
    let mut lines: Vec<Line> = Vec::new();
    let (mut crlf, mut lf) = (0usize, 0usize);
    for seg in text.split_inclusive('\n') {
        if let Some(c) = seg.strip_suffix("\r\n") {
            crlf += 1;
            lines.push((c.to_string(), "\r\n"));
        } else if let Some(c) = seg.strip_suffix('\n') {
            lf += 1;
            lines.push((c.to_string(), "\n"));
        } else if let Some(c) = seg.strip_suffix('\r') {
            // A last CRLF line cut off after its CR: it gets its LF back, not a second ending.
            crlf += 1;
            lines.push((c.to_string(), "\r\n"));
        } else {
            lines.push((seg.to_string(), ""));
        }
    }
    let eol = if crlf > lf { "\r\n" } else { "\n" };
    if let Some(last) = lines.last_mut()
        && last.1.is_empty()
    {
        last.1 = eol;
    }
    (lines, eol)
}

fn join(lines: &[Line]) -> String {
    lines.iter().flat_map(|(t, eol)| [t.as_str(), eol]).collect()
}

/// Set `table.name` to `value` (a mapping). Returns the new text.
pub fn set(text: &str, table: &str, name: &str, value: &Value) -> Result<String, String> {
    parse(text)?;
    // Lines are edited after the byte-order mark, which goes back in front as it was.
    let (bom, text) = split_bom(text);
    let (mut out, eol) = split(text);
    let lines: Vec<&str> = out.iter().map(|(t, _)| t.as_str()).collect();
    let new_lines = |rendered: String| -> Vec<Line> { rendered.lines().map(|l| (l.to_string(), eol)).collect() };
    let found = find_table(&lines, table)?;
    match found {
        None => {
            let at = lines
                .iter()
                .position(|line| line.strip_prefix("...").is_some_and(|rest| rest.is_empty() || rest.starts_with([' ', '\t'])))
                .unwrap_or(out.len());
            let mut added = vec![(format!("{table}:"), eol)];
            added.extend(new_lines(render_entry(name, value, 2)));
            out.splice(at..at, added);
        }
        Some(block) => {
            let indent = child_indent(&lines, &block);
            let rendered = new_lines(render_entry(name, value, indent));
            let child = find_child(&lines, &block, table, name)?;
            if block.end == block.header + 1
                && let Some(h) = out.get_mut(block.header)
                && let Some(rest) = key_line(&h.0, table).filter(|r| empty_inline(r))
            {
                // Keep a comment that followed the empty value.
                let comment = rest.find('#').and_then(|p| rest.get(p..)).map(|c| format!(" {c}")).unwrap_or_default();
                h.0 = format!("{table}:{comment}");
            }
            match child {
                Some((s, e)) => {
                    out.splice(s..e, rendered);
                }
                None => {
                    let at = block.end.min(out.len());
                    out.splice(at..at, rendered);
                }
            }
        }
    }
    let new = format!("{bom}{}", join(&out));
    parse(&new).map_err(|e| format!("the edited file would not parse: {e}"))?;
    Ok(new)
}

/// Remove `table.name`. Returns `Ok(None)` when it was not there, or `Err` when the file or
/// table cannot be edited safely (such as multi-document YAML or inline flow entries).
pub fn try_remove(text: &str, table: &str, name: &str) -> Result<Option<String>, String> {
    parse(text)?;
    let (bom, text) = split_bom(text);
    let (mut out, _) = split(text);
    let lines: Vec<&str> = out.iter().map(|(t, _)| t.as_str()).collect();
    let Some(block) = find_table(&lines, table)? else {
        return Ok(None);
    };
    let Some((s, e)) = find_child(&lines, &block, table, name)? else {
        return Ok(None);
    };
    out.drain(s..e);
    let new = format!("{bom}{}", join(&out));
    parse(&new).map_err(|e| format!("the edited file would not parse: {e}"))?;
    Ok(Some(new))
}

/// Remove `table.name`. Returns `None` when it was not there or could not be removed safely.
pub fn remove(text: &str, table: &str, name: &str) -> Option<String> {
    try_remove(text, table, name).ok().flatten()
}

#[cfg(test)]
mod stream_regressions {
    use super::*;
    use serde_json::json;

    /// Editing and removal reject a stream whose extra documents would be ignored by the reader.
    #[test]
    fn multiple_yaml_documents_are_refused_before_editing() {
        for text in
            ["model: first\n---\nmodel: second\n", "---\nmodel: first\n---\n", "mcp_servers:\n  craft-anything:\n    command: old\n---\nmodel: second\n"]
        {
            assert!(parse(text).is_err());
            assert!(set(text, "mcp_servers", "craft-anything", &json!({"command":"c"})).is_err());
            assert!(remove(text, "mcp_servers", "craft-anything").is_none());
        }
        assert_eq!(parse("---\nmodel: first\n...\n").unwrap()["model"], "first");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const FIXTURE: &str =
        "# my agent\nmodel: big # inline\nmcp_servers:\n  # a friend\n  other:\n    command: x\n    args: [a]\n\n# tail comment\nskills: on\n";

    #[test]
    fn insert_replace_remove_preserve_comments() {
        let entry = json!({"command": "/opt/craft-anything-cli", "args": ["mcp"], "enabled": true});
        let new = set(FIXTURE, "mcp_servers", "craft-anything", &entry).unwrap();
        assert!(new.contains("# a friend") && new.contains("# tail comment") && new.contains("model: big # inline"));
        let v = parse(&new).unwrap();
        assert_eq!(v["mcp_servers"]["craft-anything"], entry);
        assert_eq!(v["mcp_servers"]["other"]["command"], "x");
        assert_eq!(v["skills"], "on");
        let changed = json!({"command": "/new", "args": ["mcp"], "enabled": true});
        let new2 = set(&new, "mcp_servers", "craft-anything", &changed).unwrap();
        assert_eq!(parse(&new2).unwrap()["mcp_servers"]["craft-anything"], changed);
        assert_eq!(new2.matches("craft-anything").count(), 1);
        assert_eq!(remove(&new2, "mcp_servers", "craft-anything").unwrap(), FIXTURE);
        assert!(remove(FIXTURE, "mcp_servers", "craft-anything").is_none());
    }

    #[test]
    fn creates_table_and_handles_flow_style() {
        let e = json!({"command": "c", "args": []});
        let new = set("", "mcp_servers", "craft-anything", &e).unwrap();
        assert_eq!(parse(&new).unwrap()["mcp_servers"]["craft-anything"], e);
        let new = set("a: 1\n", "mcp_servers", "craft-anything", &e).unwrap();
        assert_eq!(parse(&new).unwrap()["a"], 1);
        let new = set("mcp_servers: {}\n", "mcp_servers", "craft-anything", &e).unwrap();
        assert_eq!(parse(&new).unwrap()["mcp_servers"]["craft-anything"], e);
        assert!(set("mcp_servers: {a: 1}\n", "mcp_servers", "craft-anything", &e).is_err());
        assert!(set("a: [", "mcp_servers", "x", &e).is_err());
        assert!(parse("- 1\n").is_err());
    }

    fn crlf(s: &str) -> String {
        s.replace('\n', "\r\n")
    }

    #[test]
    fn a_crlf_file_stays_crlf_and_unrelated_lines_are_byte_identical() {
        let entry = json!({"command": "/opt/craft-anything-cli", "args": ["mcp"], "enabled": true});
        let text = crlf(FIXTURE);
        let new = set(&text, "mcp_servers", "craft-anything", &entry).unwrap();
        // No bare LF anywhere: every line ends CRLF, ours included.
        assert_eq!(new.matches('\n').count(), new.matches("\r\n").count(), "{new:?}");
        // Without our block the file is exactly what it was.
        assert_eq!(remove(&new, "mcp_servers", "craft-anything").unwrap(), text);
        // Every original line is still there, unchanged.
        for l in text.split_inclusive('\n') {
            assert!(new.contains(l), "{l:?} lost in {new:?}");
        }
        assert_eq!(parse(&new).unwrap()["mcp_servers"]["craft-anything"], entry);
        // A second run changes nothing.
        assert_eq!(set(&new, "mcp_servers", "craft-anything", &entry).unwrap(), new);
        // A new table in a CRLF file, and a file with no final newline.
        let added = set("a: 1\r\nb: 2", "mcp_servers", "craft-anything", &entry).unwrap();
        assert_eq!(added.matches('\n').count(), added.matches("\r\n").count(), "{added:?}");
        assert!(added.starts_with("a: 1\r\nb: 2\r\nmcp_servers:\r\n"), "{added:?}");
        // A last line cut off after its CR gets its LF back, not "\r\r\n".
        let cut = set("a: 1\r\nb: 2\r", "mcp_servers", "craft-anything", &entry).unwrap();
        assert!(cut.starts_with("a: 1\r\nb: 2\r\nmcp_servers:\r\n"), "{cut:?}");
        assert!(!cut.contains("\r\r"), "{cut:?}");
    }

    #[test]
    fn an_lf_file_stays_lf_and_mixed_endings_are_kept_per_line() {
        let entry = json!({"command": "c", "args": ["mcp"], "enabled": true});
        let new = set(FIXTURE, "mcp_servers", "craft-anything", &entry).unwrap();
        assert!(!new.contains('\r'), "{new:?}");
        let mixed = "# top\r\nmodel: big\nmcp_servers:\r\n  other:\n    command: x\r\n";
        let new = set(mixed, "mcp_servers", "craft-anything", &entry).unwrap();
        assert!(new.starts_with("# top\r\nmodel: big\nmcp_servers:\r\n  other:\n    command: x\r\n  craft-anything:"), "{new:?}");
        assert_eq!(remove(&new, "mcp_servers", "craft-anything").unwrap(), mixed);
    }
}

#[cfg(test)]
mod document_end_regressions {
    use super::*;
    use serde_json::json;

    #[test]
    fn missing_table_is_inserted_before_document_end() {
        let value = json!({"command": "craft-anything-cli", "args": ["mcp"]});
        for text in ["model: x\n...\n", "---\nmodel: x\r\n... # end\r\n# tail\r\n"] {
            let output = set(text, "mcp_servers", "craft-anything", &value).unwrap();
            assert_eq!(parse(&output).unwrap()["mcp_servers"]["craft-anything"], value);
            assert_eq!(set(&output, "mcp_servers", "craft-anything", &value).unwrap(), output);
            let removed = remove(&output, "mcp_servers", "craft-anything").unwrap();
            assert!(removed.ends_with(text.split_once("...").map(|(_, tail)| format!("...{tail}")).as_deref().unwrap()));
            assert_eq!(parse(&removed).unwrap()["model"], "x");
        }
        let output = set("...suffix: keep\n", "mcp_servers", "craft-anything", &value).unwrap();
        assert_eq!(parse(&output).unwrap()["...suffix"], "keep");
    }
}
