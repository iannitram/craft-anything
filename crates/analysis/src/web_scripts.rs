//! Scripts from web captures: pulling them out of a capture (a page inspection result, a
//! walkthrough capture, or a HAR), planning a collision-free file layout for export, and mapping
//! generated positions back to original sources through source maps.

use craft_anything_evidence::sha256_hex;
use craft_anything_formats::har;
use craft_anything_formats::sourcemap::{self, SourceMap, base64_decode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturedScript {
    pub url: String,
    pub text: String,
}

/// Most scripts, and most bytes of script text, an extraction keeps. Scripts past it are listed
/// by URL only, so a large capture isn't copied in full when only part of it will be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScriptBudget {
    pub max_scripts: usize,
    pub max_bytes: u64,
}

impl ScriptBudget {
    pub const UNLIMITED: ScriptBudget = ScriptBudget { max_scripts: usize::MAX, max_bytes: u64::MAX };
}

/// Scripts kept within a [`ScriptBudget`], and the URLs of those left out because of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExtractedScripts {
    pub scripts: Vec<CapturedScript>,
    pub over_budget: Vec<String>,
    pub bytes: u64,
}

impl ExtractedScripts {
    /// Every script seen, kept or not.
    pub fn seen(&self) -> usize {
        self.scripts.len().saturating_add(self.over_budget.len())
    }

    /// Whether another script can be kept at all.
    fn has_room(&self, budget: ScriptBudget) -> bool {
        self.scripts.len() < budget.max_scripts
    }

    /// Whether a script of `len` bytes fits in what's left.
    fn fits(&self, budget: ScriptBudget, len: usize) -> bool {
        self.has_room(budget) && self.bytes.saturating_add(len as u64) <= budget.max_bytes
    }

    fn keep(&mut self, url: String, text: String) {
        self.bytes = self.bytes.saturating_add(text.len() as u64);
        self.scripts.push(CapturedScript { url, text });
    }
}

/// Scripts listed in a capture result: any object with a `url` and inline source
/// (`source`, `text`, `content` or `script_source`).
pub fn scripts_from_capture(v: &Value) -> Vec<CapturedScript> {
    scripts_from_capture_within(v, ScriptBudget::UNLIMITED).scripts
}

/// [`scripts_from_capture`], copying only the scripts that fit `budget`.
pub fn scripts_from_capture_within(v: &Value, budget: ScriptBudget) -> ExtractedScripts {
    let mut out = ExtractedScripts::default();
    let mut stack = vec![(v, 0usize)];
    while let Some((x, depth)) = stack.pop() {
        if depth > 10 || out.seen() >= 20_000 {
            break;
        }
        match x {
            Value::Object(m) => {
                let text = ["source", "text", "content", "script_source"].iter().find_map(|k| m.get(*k).and_then(Value::as_str));
                if let Some(text) = text {
                    let url = m.get("url").and_then(Value::as_str).unwrap_or("").to_string();
                    if out.fits(budget, text.len()) {
                        out.keep(url, text.to_string());
                    } else {
                        out.over_budget.push(url);
                    }
                    continue;
                }
                for y in m.values() {
                    stack.push((y, depth + 1));
                }
            }
            Value::Array(a) => {
                for y in a.iter().rev() {
                    stack.push((y, depth + 1));
                }
            }
            _ => {}
        }
    }
    out
}

/// JavaScript responses recorded in a HAR, when their bodies were saved.
pub fn scripts_from_har(bytes: &[u8], max_entries: usize) -> Result<Vec<CapturedScript>, String> {
    Ok(scripts_from_har_within(bytes, max_entries, ScriptBudget::UNLIMITED)?.scripts)
}

/// [`scripts_from_har`], decoding and keeping only the scripts that fit `budget`.
pub fn scripts_from_har_within(bytes: &[u8], max_entries: usize, budget: ScriptBudget) -> Result<ExtractedScripts, String> {
    let h = har::parse(bytes, max_entries).map_err(|e| e.to_string())?;
    let mut out = ExtractedScripts::default();
    for e in h.entries {
        let mime = e.response.mime.as_deref().unwrap_or("").to_ascii_lowercase();
        let path = e.request.url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
        let is_js = mime.contains("javascript") || mime.contains("ecmascript") || path.ends_with(".js") || path.ends_with(".mjs") || path.ends_with(".cjs");
        if !is_js || !(200..300).contains(&e.response.status) {
            continue;
        }
        let Some(text) = e.response.text else { continue };
        if text.is_empty() {
            continue;
        }
        if !out.has_room(budget) {
            out.over_budget.push(e.request.url);
            continue;
        }
        let text = if e.response.encoding.as_deref() == Some("base64") {
            base64_decode(&text).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
        } else {
            text
        };
        if text.is_empty() {
            continue;
        }
        if out.fits(budget, text.len()) {
            out.keep(e.request.url, text);
        } else {
            out.over_budget.push(e.request.url);
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedFile {
    pub url: String,
    /// Relative path under the export folder (`files/<host>/<path>`).
    pub path: String,
    pub sha256: String,
    pub bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// Sanitize one URL host or path segment for relative disk output, stripping leading and trailing
/// dots (including dots left at the 100-char cut) so Windows does not fold segments onto peers.
fn clean_segment(s: &str) -> String {
    let c: String = s.chars().map(|c| if c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | '@' | '+') { c } else { '_' }).take(100).collect();
    let c = c.trim_matches('.').to_string();
    if c.is_empty() { "_".into() } else { c }
}

/// Whether `file` already ends with a `.js`, `.mjs`, or `.cjs` extension (case-insensitively).
fn has_script_ext(file: &str) -> bool {
    file.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty() && !stem.ends_with('/') && (ext.eq_ignore_ascii_case("js") || ext.eq_ignore_ascii_case("mjs") || ext.eq_ignore_ascii_case("cjs"))
    })
}

struct LayoutCandidate {
    sha: String,
    rel: String,
    hashed_base: String,
    reason: Option<String>,
}

/// Plan where each script goes. Same URL path seen twice (query variants, different content)
/// gets a content-hash suffix and a stated reason.
pub fn plan_layout(scripts: &[CapturedScript]) -> Vec<PlannedFile> {
    use craft_anything_js::recover::{ancestor_dir_keys, module_path_key};

    let candidates: Vec<LayoutCandidate> = scripts
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let sha = sha256_hex(s.text.as_bytes());
            let short = sha.get(..10).unwrap_or(&sha).to_string();
            let (host, path, query) = match s.url.split_once("://") {
                Some((scheme, rest)) if !rest.is_empty() => {
                    let rest = rest.split('#').next().unwrap_or(rest);
                    let (hp, q) = rest.split_once('?').map_or((rest, None), |(a, b)| (a, Some(b)));
                    let (h, p) = hp.split_once('/').unwrap_or((hp, ""));
                    let h = h.rsplit('@').next().unwrap_or(h);
                    (if scheme == "file" { "local".to_string() } else { clean_segment(h) }, p.to_string(), q.is_some())
                }
                _ => ("inline".to_string(), format!("script-{i}.js"), false),
            };
            let mut segs: Vec<String> = path.split('/').filter(|p| !p.is_empty() && *p != "." && *p != "..").map(clean_segment).take(24).collect();
            if segs.is_empty() {
                segs.push("index.js".into());
            }
            let mut file = segs.join("/");
            if !has_script_ext(&file) {
                file.push_str(".js");
            }
            let mut rel = format!("files/{host}/{file}");
            let mut reason = None;
            let hashed_base = if query {
                rel = with_suffix(&rel, &short);
                reason = Some("URL has a query string; named by content hash to keep variants apart".to_string());
                rel.clone()
            } else {
                with_suffix(&rel, &short)
            };
            LayoutCandidate { sha, rel, hashed_base, reason }
        })
        .collect();

    let mut used: BTreeSet<String> = candidates.iter().flat_map(|c| ancestor_dir_keys(&c.rel)).collect();
    let mut out = Vec::new();
    for (s, c) in scripts.iter().zip(candidates) {
        let LayoutCandidate { sha, mut rel, hashed_base, mut reason } = c;
        if used.contains(&module_path_key(&rel)) {
            rel = hashed_base.clone();
            reason = Some("another script already used this path; named by content hash".to_string());
            let mut n = 2usize;
            while used.contains(&module_path_key(&rel)) {
                rel = with_suffix(&hashed_base, &n.to_string());
                n += 1;
            }
        }
        used.insert(module_path_key(&rel));
        out.push(PlannedFile { url: s.url.clone(), path: rel, sha256: sha, bytes: s.text.len(), reason });
    }
    out
}

fn with_suffix(path: &str, suffix: &str) -> String {
    match path.rsplit_once('.') {
        Some((stem, ext)) if !stem.ends_with('/') => format!("{stem}.{suffix}.{ext}"),
        _ => format!("{path}.{suffix}"),
    }
}

/// Map a generated position (1-based line, 0-based column) to original positions, with a few
/// lines of original source around each when the map embeds it.
pub fn trace_location(map: &SourceMap, line: u32, column: u32, context: u32) -> Value {
    let line0 = line.saturating_sub(1);
    let positions = map.original_positions(line0, column);
    let leaf = leaf_map(map, line0, column);
    let rows: Vec<Value> = positions
        .iter()
        .map(|p| {
            let snippet = leaf.source_content(p.source_index).map(|text| {
                let first = p.line.saturating_sub(context);
                text.lines().enumerate().skip(first as usize).take((context * 2 + 1) as usize).map(|(i, l)| json!({"line": i + 1, "text": l.chars().take(400).collect::<String>()})).collect::<Vec<_>>()
            });
            json!({"source": p.source, "line": p.line + 1, "column": p.column, "name": p.name, "generated_column": p.generated_column, "context": snippet, "ignored": leaf.ignore_list.contains(&p.source_index)})
        })
        .collect();
    json!({"generated": {"line": line, "column": column}, "original_positions": rows, "found": !rows.is_empty(), "sources": source_count(map), "segments": map.segments})
}

/// Resolve the leaf section map covering 0-based `(line, column)`, or `map` itself when flat.
fn leaf_map(mut map: &SourceMap, mut line: u32, mut column: u32) -> &SourceMap {
    while !map.sections.is_empty() {
        let end = map.sections.partition_point(|(l, c, _)| (*l, *c) <= (line, column));
        let Some((l, c, child)) = end.checked_sub(1).and_then(|i| map.sections.get(i)) else {
            return map;
        };
        let rel_line = line.saturating_sub(*l);
        column = if rel_line == 0 { column.saturating_sub(*c) } else { column };
        line = rel_line;
        map = child;
    }
    map
}

/// Count total source entries across `map` and any inline sections.
fn source_count(map: &SourceMap) -> usize {
    map.sections.iter().fold(map.sources.len(), |n, (_, _, sub)| n.saturating_add(source_count(sub)))
}

/// Parse a map from bytes (raw JSON or a `data:` URL's payload).
pub fn parse_map(bytes: &[u8]) -> Result<SourceMap, String> {
    let text = std::str::from_utf8(bytes).unwrap_or("");
    if let Some(decoded) = text.trim().starts_with("data:").then(|| sourcemap::decode_data_url(text.trim())).flatten() {
        return sourcemap::parse(&decoded, sourcemap::Limits::default()).map_err(|e| e.to_string());
    }
    sourcemap::parse(bytes, sourcemap::Limits::default()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_scripts_and_layout() {
        let cap = json!({"page": {"scripts": [
            {"url": "https://ex.test/assets/app.js?v=1", "source": "a()"},
            {"url": "https://ex.test/assets/app.js?v=2", "source": "b()"},
            {"url": "https://ex.test/../../etc/passwd", "source": "c()"},
            {"url": "", "source": "inline()"},
            {"url": "https://ex.test/assets/app.js", "source": "d()"},
            {"url": "https://ex.test/assets/app.js", "source": "e()"},
        ]}});
        let s = scripts_from_capture(&cap);
        assert_eq!(s.len(), 6);
        let plan = plan_layout(&s);
        let paths: BTreeSet<&str> = plan.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths.len(), 6, "{plan:?}");
        assert!(plan.iter().all(|p| !p.path.contains("..") && p.path.starts_with("files/")));
        assert!(plan[0].reason.is_some());
        assert_eq!(plan[2].path, "files/ex.test/etc/passwd.js");
        assert_eq!(plan[3].path, "files/inline/script-3.js");
        assert_eq!(plan[4].path, "files/ex.test/assets/app.js");
        assert!(plan[5].reason.as_deref().unwrap().contains("already used"));
    }

    #[test]
    fn har_scripts() {
        let har = br#"{"log":{"entries":[{"request":{"method":"GET","url":"https://x.test/a.js"},"response":{"status":200,"content":{"mimeType":"text/javascript","text":"YSgp","encoding":"base64"}}},
          {"request":{"method":"GET","url":"https://x.test/b.css"},"response":{"status":200,"content":{"mimeType":"text/css","text":"x"}}}]}}"#;
        let s = scripts_from_har(har, 100).unwrap();
        assert_eq!(s, vec![CapturedScript { url: "https://x.test/a.js".into(), text: "a()".into() }]);
    }

    #[test]
    fn extraction_keeps_only_what_fits_the_budget() {
        let cap = json!({"scripts": [
            {"url": "https://ex.test/a.js", "source": "aaaa"},
            {"url": "https://ex.test/big.js", "source": "bbbbbbbbbb"},
            {"url": "https://ex.test/c.js", "source": "cc"},
            {"url": "https://ex.test/d.js", "source": "d"},
        ]});
        let e = scripts_from_capture_within(&cap, ScriptBudget { max_scripts: 10, max_bytes: 6 });
        let kept: Vec<&str> = e.scripts.iter().map(|s| s.url.as_str()).collect();
        assert_eq!(kept, ["https://ex.test/a.js", "https://ex.test/c.js"]);
        assert_eq!(e.over_budget, ["https://ex.test/big.js", "https://ex.test/d.js"]);
        assert_eq!((e.bytes, e.seen()), (6, 4));
        let e = scripts_from_capture_within(&cap, ScriptBudget { max_scripts: 1, max_bytes: u64::MAX });
        assert_eq!((e.scripts.len(), e.over_budget.len()), (1, 3));

        let har = br#"{"log":{"entries":[{"request":{"method":"GET","url":"https://x.test/a.js"},"response":{"status":200,"content":{"mimeType":"text/javascript","text":"YSgp","encoding":"base64"}}},
          {"request":{"method":"GET","url":"https://x.test/b.js"},"response":{"status":200,"content":{"mimeType":"text/javascript","text":"b()"}}}]}}"#;
        let e = scripts_from_har_within(har, 100, ScriptBudget { max_scripts: 1, max_bytes: u64::MAX }).unwrap();
        assert_eq!(e.scripts, vec![CapturedScript { url: "https://x.test/a.js".into(), text: "a()".into() }]);
        assert_eq!(e.over_budget, ["https://x.test/b.js"]);
    }

    #[test]
    fn traces_with_context() {
        let m = parse_map(
            br#"{"version":3,"sources":["src/a.ts"],"names":["save"],"mappings":";AAEAA","sourcesContent":["// one\n// two\nfunction save() {}\n"]}"#,
        )
        .unwrap();
        let t = trace_location(&m, 2, 0, 1);
        assert_eq!(t["original_positions"][0]["source"], "src/a.ts");
        assert_eq!(t["original_positions"][0]["line"], 3);
        assert_eq!(t["original_positions"][0]["name"], "save");
        assert_eq!(t["original_positions"][0]["context"][1]["text"], "function save() {}");
        assert_eq!(trace_location(&m, 1, 0, 1)["found"], false);
    }
}
