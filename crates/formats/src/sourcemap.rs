//! Shared bounded Source Map v3 core; annotation/data URL handling stays app-owned.
pub use craft_sourcemap::{InventoryLimits, Limits, OriginalPosition, Segment, SourceInventory, SourceMap, SourceMapError, parse, parse_sources};

/// Find a `sourceMappingURL` on a comment-only line near the end of a script.
pub fn find_source_mapping_url(js: &str) -> Option<String> {
    let terminators = ['\n', '\r', '\u{2028}', '\u{2029}'];
    // Ignore one trailing terminator, and count CRLF as a single line break.
    let mut rest = js.strip_suffix("\r\n").or_else(|| js.strip_suffix(terminators)).unwrap_or(js);
    for _ in 0..64 {
        let (line, preceding) = if let Some((i, separator)) = rest.rmatch_indices(terminators).next() {
            let before = rest.get(..i)?;
            let before = if separator == "\n" { before.strip_suffix('\r').unwrap_or(before) } else { before };
            (rest.get(i + separator.len()..)?, Some(before))
        } else {
            (rest, None)
        };
        if let Some(url) = source_mapping_url_on_line(line) {
            return Some(url.to_string());
        }
        let Some(preceding) = preceding else { break };
        rest = preceding;
    }
    None
}

/// Keep the last valid annotation on a line of comments. A code token ends the search on this line.
fn source_mapping_url_on_line(mut line: &str) -> Option<&str> {
    let mut url = None;
    loop {
        line = line.trim_start();
        if line.is_empty() {
            return url;
        }
        if let Some(comment) = line.strip_prefix("//") {
            return source_mapping_url_in_comment(comment).or(url);
        }
        let block = line.strip_prefix("/*")?;
        let Some((comment, after)) = block.split_once("*/") else { return url };
        url = source_mapping_url_in_comment(comment).or(url);
        line = after;
    }
}

/// Read a nonempty URL from a complete comment's annotation.
fn source_mapping_url_in_comment(comment: &str) -> Option<&str> {
    let url = comment.strip_prefix(['#', '@'])?.trim_start().strip_prefix("sourceMappingURL=")?.trim();
    (!url.is_empty() && !url.chars().any(char::is_whitespace)).then_some(url)
}

/// Decode a `data:` URL, including percent escapes and optional base64 content.
pub fn decode_data_url(url: &str) -> Option<Vec<u8>> {
    let (scheme, rest) = url.split_once(':')?;
    if !scheme.eq_ignore_ascii_case("data") {
        return None;
    }
    let rest = rest.split_once('#').map_or(rest, |(body, _)| body);
    let (meta, payload) = rest.split_once(',')?;
    let decoded: std::borrow::Cow<'_, [u8]> = if payload.contains('%') {
        let hex = |b: u8| match b {
            b'0'..=b'9' => Some(b - b'0'),
            b'a'..=b'f' => Some(b - b'a' + 10),
            b'A'..=b'F' => Some(b - b'A' + 10),
            _ => None,
        };
        let mut decoded = Vec::with_capacity(payload.len());
        let mut bytes = payload.bytes();
        while let Some(b) = bytes.next() {
            decoded.push(if b == b'%' { (hex(bytes.next()?)? << 4) | hex(bytes.next()?)? } else { b });
        }
        std::borrow::Cow::Owned(decoded)
    } else {
        std::borrow::Cow::Borrowed(payload.as_bytes())
    };
    if meta.rsplit_once(';').is_some_and(|(_, flag)| flag.trim_matches(|c: char| c.is_ascii_whitespace()).eq_ignore_ascii_case("base64")) {
        base64_decode(std::str::from_utf8(&decoded).ok()?)
    } else {
        Some(decoded.into_owned())
    }
}

/// Standard or URL-safe base64, padding optional. `None` on any invalid character.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    crate::bytes::decode_base64(text, true)
}

#[cfg(test)]
#[path = "sourcemap_legacy.rs"]
mod legacy;

#[cfg(test)]
mod shared_parity {
    use super::*;
    #[test]
    fn core_errors_inventory_and_positions_match() {
        let samples: &[&[u8]] = &[b"", b"{}", br#"{"version":3,"sources":["a","b"],"mappings":"EAAA,ACAA,E"}"#, br#"{"version":3,"sourceRoot":"webpack:///","sources":["a"],"mappings":"AAAA"}"#, br#"{"version":3,"sections":[{"offset":{"line":0,"column":0},"map":{"version":3,"sources":["a"],"mappings":"AAAA"}},{"offset":{"line":1,"column":0},"url":"external"}]}"#, br#"{"version":3,"sources":[null],"names":[],"mappings":"AAAAC"}"#];
        for data in samples {
            for cap in [0, 1, 8, 4096] {
                let old_limit = legacy::Limits { max_bytes: cap, max_segments: cap, max_lines: cap, max_depth: 8, max_entries: cap };
                let limit = Limits { max_bytes: cap, max_segments: cap, max_lines: cap, max_depth: 8, max_entries: cap };
                for sources_only in [false, true] {
                    let old = if sources_only { legacy::parse_sources(data, old_limit) } else { legacy::parse(data, old_limit) };
                    let new = if sources_only { parse_sources(data, limit) } else { parse(data, limit) };
                    match (old, new) {
                        (Ok(a), Ok(b)) => {
                            assert_eq!(format!("{a:?}"), format!("{b:?}"));
                            assert_eq!(a.all_sources(), b.all_sources());
                            for line in 0..3 {
                                for col in [0, 1, 3, 4, u32::MAX] {
                                    assert_eq!(format!("{:?}", a.original_positions(line, col)), format!("{:?}", b.original_positions(line, col)));
                                }
                            }
                        }
                        (Err(a), Err(b)) => assert_eq!(format!("{a:?}"), format!("{b:?}")),
                        _ => panic!("source map parity mismatch"),
                    }
                }
            }
        }
    }
}
