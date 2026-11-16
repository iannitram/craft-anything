//! Small helpers shared by the extractors.

use sha2::{Digest, Sha256};

/// Byte offset → 1-based line number.
#[derive(Debug, Clone)]
pub struct LineIndex {
    starts: Vec<u32>,
}

impl LineIndex {
    /// Index JavaScript line terminators, treating CRLF as one ending.
    pub fn new(src: &str) -> Self {
        let mut starts = vec![0u32];
        let mut chars = src.char_indices().peekable();
        while let Some((i, c)) = chars.next() {
            let mut end = i + c.len_utf8();
            if c == '\r' && chars.peek().is_some_and(|(_, next)| *next == '\n') {
                chars.next();
                end += 1;
            }
            if matches!(c, '\r' | '\n' | '\u{2028}' | '\u{2029}') {
                starts.push(u32::try_from(end).unwrap_or(u32::MAX));
            }
        }
        LineIndex { starts }
    }

    pub fn line(&self, offset: u32) -> u32 {
        let idx = self.starts.partition_point(|s| *s <= offset);
        u32::try_from(idx.max(1)).unwrap_or(u32::MAX)
    }

    pub fn count(&self) -> u32 {
        u32::try_from(self.starts.len()).unwrap_or(u32::MAX)
    }
}

/// Push when absent and below `cap`.
pub fn push_unique(v: &mut Vec<String>, s: String, cap: usize) {
    if v.len() < cap && !v.contains(&s) {
        v.push(s);
    }
}

pub fn sha256_hex(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

/// Byte slice of `src` by offsets, or "" when out of range or not on character boundaries.
pub fn slice(src: &str, start: u32, end: u32) -> &str {
    src.get(start as usize..end as usize).unwrap_or("")
}
