//! HTML pages: script references, inline scripts, module preloads and the Content Security Policy.
//! A tolerant tag scanner, not a full HTML parser.

use serde::{Deserialize, Serialize};

const MAX_TAGS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HtmlScript {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
    pub module: bool,
    /// Byte range of inline script text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline: Option<(u32, u32)>,
    pub line: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HtmlFacts {
    pub scripts: Vec<HtmlScript>,
    pub preloads: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub csp: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Attributes of the tag starting at `start` (just after the tag name). Returns (attrs, end of tag).
fn attributes(src: &str, lower: &str, mut i: usize) -> (Vec<(String, String)>, usize) {
    let b = lower.as_bytes();
    let mut out = Vec::new();
    while i < b.len() {
        while b.get(i).is_some_and(|c| c.is_ascii_whitespace() || *c == b'/') {
            i += 1;
        }
        match b.get(i) {
            None => break,
            Some(b'>') => return (out, i + 1),
            _ => {}
        }
        let ns = i;
        while b.get(i).is_some_and(|c| !c.is_ascii_whitespace() && *c != b'=' && *c != b'>') {
            i += 1;
        }
        let name = lower.get(ns..i).unwrap_or("").to_string();
        while b.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        let mut value = String::new();
        if b.get(i) == Some(&b'=') {
            i += 1;
            while b.get(i).is_some_and(u8::is_ascii_whitespace) {
                i += 1;
            }
            match b.get(i) {
                Some(q @ (b'"' | b'\'')) => {
                    let q = *q;
                    let vs = i + 1;
                    i = vs;
                    while b.get(i).is_some_and(|c| *c != q) {
                        i += 1;
                    }
                    value = src.get(vs..i).unwrap_or("").to_string();
                    i += 1;
                }
                _ => {
                    let vs = i;
                    while b.get(i).is_some_and(|c| !c.is_ascii_whitespace() && *c != b'>') {
                        i += 1;
                    }
                    value = src.get(vs..i).unwrap_or("").to_string();
                }
            }
        }
        if name.is_empty() {
            i += 1;
        } else if out.len() < 64 {
            out.push((name, decode_entities(&value)));
        }
    }
    (out, i.min(b.len()))
}

fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find('&') {
        out.push_str(rest.get(..at).unwrap_or_default());
        rest = rest.get(at + 1..).unwrap_or_default();
        let decoded = rest
            .strip_prefix('#')
            .and_then(|number| {
                let (digits, radix) = number.strip_prefix(['x', 'X']).map_or((number, 10), |n| (n, 16));
                let count = digits.bytes().take_while(|c| if radix == 16 { c.is_ascii_hexdigit() } else { c.is_ascii_digit() }).count();
                if count == 0 {
                    return None;
                }
                let value = digits.get(..count).and_then(|s| u32::from_str_radix(s, radix).ok()).unwrap_or(u32::MAX);
                let consumed = rest.len() - digits.len() + count;
                let consumed = consumed + usize::from(rest.as_bytes().get(consumed) == Some(&b';'));
                // HTML maps the numeric C1 references through the Windows-1252 table.
                const C1: [u32; 32] = [
                    0x20ac, 0x81, 0x201a, 0x192, 0x201e, 0x2026, 0x2020, 0x2021, 0x2c6, 0x2030, 0x160, 0x2039, 0x152, 0x8d, 0x17d, 0x8f, 0x90, 0x2018, 0x2019,
                    0x201c, 0x201d, 0x2022, 0x2013, 0x2014, 0x2dc, 0x2122, 0x161, 0x203a, 0x153, 0x9d, 0x17e, 0x178,
                ];
                let value = value.checked_sub(0x80).and_then(|i| C1.get(i as usize)).copied().unwrap_or(value);
                Some((char::from_u32(value).filter(|c| *c != '\0').unwrap_or('\u{fffd}'), consumed))
            })
            .or_else(|| {
                [("amp;", '&'), ("quot;", '"'), ("apos;", '\''), ("lt;", '<'), ("gt;", '>')]
                    .into_iter()
                    .find_map(|(name, value)| rest.starts_with(name).then_some((value, name.len())))
            });
        if let Some((c, consumed)) = decoded {
            out.push(c);
            rest = rest.get(consumed..).unwrap_or_default();
        } else {
            out.push('&');
        }
    }
    out.push_str(rest);
    out
}

fn get<'v>(attrs: &'v [(String, String)], k: &str) -> Option<&'v str> {
    attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

/// Find an actual closing tag, not a longer name such as `</scripture>`.
fn closing_tag(lower: &str, from: usize, name: &str) -> Option<usize> {
    let needle = format!("</{name}");
    lower.get(from..)?.match_indices(&needle).find_map(|(offset, _)| {
        let at = from + offset;
        lower.as_bytes().get(at + needle.len()).filter(|c| c.is_ascii_whitespace() || matches!(c, b'/' | b'>')).map(|_| at)
    })
}

/// Scan an HTML document.
pub fn scan_html(src: &str) -> HtmlFacts {
    let lower = src.to_ascii_lowercase();
    let lines = crate::util::LineIndex::new(src);
    let mut out = HtmlFacts::default();
    let mut pos = 0usize;
    let mut tags = 0usize;
    while let Some(rel) = lower.get(pos..).and_then(|r| r.find('<')) {
        tags += 1;
        if tags > MAX_TAGS * 50 || out.scripts.len() >= MAX_TAGS {
            break;
        }
        let at = pos + rel;
        let rest = lower.get(at + 1..).unwrap_or("");
        // Inspect only the names we handle: malformed runs of `<` must not rescan the remaining document.
        let name = ["script", "link", "meta", "title", "style", "textarea", "xmp", "iframe", "noembed", "noframes", "plaintext"]
            .into_iter()
            .find(|name| rest.starts_with(*name) && rest.as_bytes().get(name.len()).is_none_or(|c| c.is_ascii_whitespace() || matches!(*c, b'/' | b'>')))
            .unwrap_or_default();
        let after = at + 1 + name.len();
        match name {
            "script" => {
                let (attrs, end) = attributes(src, &lower, after);
                let close = closing_tag(&lower, end, "script").unwrap_or(lower.len());
                let module = get(&attrs, "type").is_some_and(|t| t.eq_ignore_ascii_case("module"));
                let line = lines.line(u32::try_from(at).unwrap_or(u32::MAX));
                let src_attr = get(&attrs, "src").map(String::from);
                let js = javascript_type(get(&attrs, "type"));
                let inline = (src_attr.is_none() && close > end && js && src.get(end..close).is_some_and(|t| !t.trim().is_empty()))
                    .then(|| (u32::try_from(end).unwrap_or(u32::MAX), u32::try_from(close).unwrap_or(u32::MAX)));
                if js && (src_attr.is_some() || inline.is_some()) {
                    out.scripts.push(HtmlScript { src: src_attr, module, inline, line });
                }
                pos = close.max(end).max(at + 1);
            }
            "style" | "textarea" | "xmp" | "iframe" | "noembed" | "noframes" => {
                let (_, end) = attributes(src, &lower, after);
                pos = closing_tag(&lower, end, name).unwrap_or(lower.len()).max(at + 1);
            }
            "plaintext" => break,
            "link" => {
                let (attrs, end) = attributes(src, &lower, after);
                let rel_attr = get(&attrs, "rel").unwrap_or("").to_ascii_lowercase();
                let script_destination = get(&attrs, "as").is_some_and(|value| value.eq_ignore_ascii_case("script"));
                if rel_attr.split_ascii_whitespace().any(|token| token == "modulepreload" || (token == "preload" && script_destination))
                    && let Some(h) = get(&attrs, "href")
                    && out.preloads.len() < MAX_TAGS
                {
                    out.preloads.push(h.to_string());
                }
                pos = end.max(at + 1);
            }
            "meta" => {
                let (attrs, end) = attributes(src, &lower, after);
                if get(&attrs, "http-equiv").is_some_and(|h| h.eq_ignore_ascii_case("content-security-policy")) {
                    out.csp = get(&attrs, "content").map(String::from);
                }
                pos = end.max(at + 1);
            }
            "title" => {
                let (_, end) = attributes(src, &lower, after);
                let close = closing_tag(&lower, end, "title").unwrap_or(lower.len());
                out.title = src.get(end..close).map(|t| decode_entities(t).trim().chars().take(200).collect());
                pos = close.max(at + 1);
            }
            _ if rest.starts_with("!--") => {
                pos = lower.get(at..).and_then(|r| r.find("-->")).map_or(lower.len(), |c| at + c + 3);
            }
            _ if rest.as_bytes().first().is_some_and(u8::is_ascii_alphabetic)
                || rest.strip_prefix('/').and_then(|s| s.as_bytes().first()).is_some_and(u8::is_ascii_alphabetic) =>
            {
                let (_, end) = attributes(src, &lower, after);
                pos = end.max(at + 1);
            }
            _ => pos = at + 1,
        }
    }
    out
}

/// Whether a `<script type>` is code the page runs: no type, a JavaScript MIME type or `module`
/// (and JSX for in-browser Babel, which is code too). Import maps, speculation rules and data
/// blocks such as JSON or HTML templates aren't JavaScript (#285).
fn javascript_type(t: Option<&str>) -> bool {
    let Some(t) = t else { return true };
    let essence = t.trim_matches(|c: char| c.is_ascii_whitespace()).to_ascii_lowercase();
    essence.is_empty()
        || essence == "module"
        || matches!(essence.as_str(), "text/babel" | "text/jsx" | "text/jscript" | "text/livescript")
        || [
            "text/javascript",
            "text/javascript1.0",
            "text/javascript1.1",
            "text/javascript1.2",
            "text/javascript1.3",
            "text/javascript1.4",
            "text/javascript1.5",
            "application/javascript",
            "application/ecmascript",
            "text/ecmascript",
            "application/x-javascript",
            "application/x-ecmascript",
            "text/x-javascript",
            "text/x-ecmascript",
        ]
        .iter()
        .any(|m| essence == *m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_scripts_and_csp() {
        let h = r#"<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="default-src 'self'">
<title>Notes</title><link rel="modulepreload" href="/assets/vendor-abc.js">
<script type="module" src="./app.js"></script><!-- <script src="no.js"></script> -->
<script>window.x = 1;</script><script type="application/json">{}</script></head></html>"#;
        let f = scan_html(h);
        assert_eq!(f.scripts.len(), 2);
        assert_eq!(f.scripts[0].src.as_deref(), Some("./app.js"));
        assert!(f.scripts[0].module);
        let (s, e) = f.scripts[1].inline.unwrap();
        assert_eq!(&h[s as usize..e as usize], "window.x = 1;");
        assert_eq!(f.preloads, vec!["/assets/vendor-abc.js"]);
        assert_eq!(f.csp.as_deref(), Some("default-src 'self'"));
        assert_eq!(f.title.as_deref(), Some("Notes"));
    }

    /// #285: import maps, speculation rules and HTML templates were read as JavaScript.
    #[test]
    fn only_javascript_types_are_scripts() {
        for t in ["importmap", "speculationrules", "text/html", "application/json", "text/x-template"] {
            let f = scan_html(&format!(r#"<script type="{t}">{{"imports": {{}}}}</script><script type="{t}" src="data.json"></script>"#));
            assert!(f.scripts.is_empty(), "{t}: {:?}", f.scripts);
        }
        for t in ["", "module", "text/javascript", "Application/JavaScript", "text/babel"] {
            assert_eq!(scan_html(&format!(r#"<script type="{t}">fetch("/real-api");</script>"#)).scripts.len(), 1, "{t}");
        }
    }

    #[test]
    fn junk_does_not_panic() {
        for s in ["<", "<script", "<script src=", "<script src='x", "<<<<>>>", "<script>é", "<meta http-equiv=content-security-policy content=\"é", "<!--"] {
            let _ = scan_html(s);
        }
    }
}

#[cfg(test)]
mod title_entities_regression {
    use super::scan_html;

    #[test]
    fn displayed_title_decodes_references_once() {
        let facts = scan_html("<title>Fish &amp; Chips &#x1f41f; &amp;quot;</title>");
        assert_eq!(facts.title.as_deref(), Some("Fish & Chips 🐟 &quot;"));
        assert_eq!(scan_html("<title>A &unknown; B</title>").title.as_deref(), Some("A &unknown; B"));
    }

    #[test]
    fn title_limit_counts_displayed_characters() {
        let html = format!("<title>{}</title>", "&amp;".repeat(201));
        assert_eq!(scan_html(&html).title, Some("&".repeat(200)));
    }
}
