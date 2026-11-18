//! Checked little/big-endian reads over untrusted bytes. Every read returns `None` when out of range.

#[derive(Debug, Clone, Copy)]
pub struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    pub fn slice(&self, off: usize, len: usize) -> Option<&'a [u8]> {
        let end = off.checked_add(len)?;
        self.data.get(off..end)
    }

    pub fn u8(&self, off: usize) -> Option<u8> {
        self.data.get(off).copied()
    }

    fn arr<const N: usize>(&self, off: usize) -> Option<[u8; N]> {
        self.slice(off, N)?.try_into().ok()
    }

    pub fn u16_le(&self, off: usize) -> Option<u16> {
        self.arr(off).map(u16::from_le_bytes)
    }

    pub fn u32_le(&self, off: usize) -> Option<u32> {
        self.arr(off).map(u32::from_le_bytes)
    }

    pub fn u64_le(&self, off: usize) -> Option<u64> {
        self.arr(off).map(u64::from_le_bytes)
    }

    pub fn u16_be(&self, off: usize) -> Option<u16> {
        self.arr(off).map(u16::from_be_bytes)
    }

    pub fn u32_be(&self, off: usize) -> Option<u32> {
        self.arr(off).map(u32::from_be_bytes)
    }

    pub fn u64_be(&self, off: usize) -> Option<u64> {
        self.arr(off).map(u64::from_be_bytes)
    }

    /// A NUL-terminated string starting at `off`, at most `max` bytes, lossily decoded.
    pub fn cstr(&self, off: usize, max: usize) -> Option<String> {
        let rest = self.data.get(off..)?;
        let rest = rest.get(..max.min(rest.len()))?;
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        Some(String::from_utf8_lossy(rest.get(..end)?).into_owned())
    }

    /// Unsigned LEB128 at `off`: (value, bytes consumed). At most 10 bytes.
    pub fn uleb128(&self, off: usize) -> Option<(u64, usize)> {
        let mut result = 0u64;
        for i in 0..10usize {
            let b = self.u8(off.checked_add(i)?)?;
            let payload = u64::from(b & 0x7f);
            let shift = 7 * i as u32;
            if shift > 0 && payload.checked_shl(shift)? >> shift != payload {
                return None;
            }
            result |= payload << shift;
            if b & 0x80 == 0 {
                return Some((result, i + 1));
            }
        }
        None
    }
}

#[cfg(test)]
/// Decode one modified UTF-8 code unit. Invalid bytes consume only themselves,
/// so a damaged sequence cannot hide valid text or borrow missing bytes.
// Only the frozen DEX/Java parity oracles still use the local decoder.
#[cfg(test)]
pub(crate) fn mutf8_unit(bytes: &[u8]) -> (u16, usize) {
    match bytes {
        [a @ 0x00..=0x7f, ..] => (u16::from(*a), 1),
        [0xc0, 0x80, ..] => (0, 2),
        [a @ 0xc2..=0xdf, b @ 0x80..=0xbf, ..] => ((u16::from(*a & 0x1f) << 6) | u16::from(*b & 0x3f), 2),
        [a @ 0xe0..=0xef, b @ 0x80..=0xbf, c @ 0x80..=0xbf, ..] if *a != 0xe0 || *b >= 0xa0 => {
            ((u16::from(*a & 0x0f) << 12) | (u16::from(*b & 0x3f) << 6) | u16::from(*c & 0x3f), 3)
        }
        _ => (0xfffd, 1),
    }
}

/// Decode base64 with optional padding and ASCII whitespace.
/// Padding must finish the last quantum; data after it is never ignored.
pub(crate) fn decode_base64(text: &str, url_safe: bool) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut acc = 0u32;
    let mut bits = 0u32;
    let mut digits = 0usize;
    let mut padding = 0usize;
    for c in text.bytes() {
        match c {
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            b'=' => {
                padding += 1;
                if padding > 2 {
                    return None;
                }
                continue;
            }
            _ if padding != 0 => return None,
            _ => {}
        }
        let value = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'-' if url_safe => 62,
            b'_' if url_safe => 63,
            _ => return None,
        };
        acc = (acc << 6) | u32::from(value);
        bits += 6;
        digits += 1;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    let remainder = digits % 4;
    if remainder == 1 || (padding != 0 && !matches!((remainder, padding), (2, 2) | (3, 1))) {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_are_checked() {
        let r = Reader::new(&[1, 2, 3]);
        assert_eq!(r.u16_le(1), Some(0x0302));
        assert_eq!(r.u32_le(0), None);
        assert_eq!(r.slice(usize::MAX, 2), None);
        assert_eq!(r.cstr(0, 10).as_deref(), Some("\u{1}\u{2}\u{3}"));
        assert_eq!(Reader::new(&[0xe5, 0x8e, 0x26]).uleb128(0), Some((624485, 3)));
        assert_eq!(Reader::new(&[0x80; 12]).uleb128(0), None);
    }

    #[test]
    fn uleb128_rejects_overflow() {
        let mut v = vec![0x80; 9];
        v.push(0x7f);
        assert_eq!(Reader::new(&v).uleb128(0), None);
        let mut v = vec![0x80; 9];
        v.push(0x02);
        assert_eq!(Reader::new(&v).uleb128(0), None);
        let mut v = vec![0xff; 9];
        v.push(0x01);
        assert_eq!(Reader::new(&v).uleb128(0), Some((u64::MAX, 10)));
    }
}
