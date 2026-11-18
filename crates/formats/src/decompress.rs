//! Bounded, pure-Rust decompressors for the stream formats a firmware image commonly carries:
//! xz, standalone LZMA (`.lzma`), bzip2 and Zstandard. Each decoder caps its output at `max_out`
//! bytes (returning `TooLarge` rather than allocating without limit) and never panics on a corrupt
//! or truncated stream. gzip/zlib/zip live in `firmware`; these complete the set.
//!
//! All three backing crates (`lzma-rs`, `bzip2-rs`, `ruzstd`) are pure Rust and build for
//! `wasm32-unknown-unknown`.

use std::io::{self, Read, Write};

/// Why a decode stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DecompressError {
    #[error("the compressed stream is damaged")]
    Corrupt,
    #[error("the compressed stream ends early")]
    Truncated,
    #[error("the output would exceed the size limit")]
    TooLarge,
}

/// A `Write` sink that collects at most `cap` bytes, then refuses the rest. The `exceeded` flag lets
/// the caller tell a size-limit stop from a genuine decode error.
struct CapWrite {
    buf: Vec<u8>,
    cap: usize,
    exceeded: bool,
}

impl CapWrite {
    fn new(cap: usize) -> Self {
        CapWrite { buf: Vec::new(), cap, exceeded: false }
    }
}

impl Write for CapWrite {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.cap {
            self.exceeded = true;
            return Err(io::Error::other("output size limit reached"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn cap_of(max_out: u64) -> usize {
    usize::try_from(max_out).unwrap_or(usize::MAX)
}

/// Read a `Read` source into a `Vec`, capped at `cap` bytes.
fn read_capped(mut src: impl Read, cap: usize) -> Result<Vec<u8>, DecompressError> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        match src.read(&mut chunk) {
            Ok(0) => return Ok(out),
            Ok(n) => {
                let slice = chunk.get(..n).unwrap_or(&[]);
                if out.len().saturating_add(slice.len()) > cap {
                    return Err(DecompressError::TooLarge);
                }
                out.extend_from_slice(slice);
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(DecompressError::Corrupt),
        }
    }
}

/// Decompress every xz stream, including standard stream padding, within one output budget.
pub fn xz(input: &[u8], max_out: u64) -> Result<Vec<u8>, DecompressError> {
    let ranges = xz_stream_ranges(input, max_out)?;
    let mut out = CapWrite::new(cap_of(max_out));
    for range in ranges.into_iter().rev() {
        let stream = input.get(range).ok_or(DecompressError::Truncated)?;
        let mut cursor = io::Cursor::new(stream);
        match lzma_rs::xz_decompress(&mut cursor, &mut out) {
            Ok(()) => {}
            Err(_) if out.exceeded => return Err(DecompressError::TooLarge),
            Err(lzma_rs::error::Error::HeaderTooShort(_)) => return Err(DecompressError::Truncated),
            Err(_) => return Err(DecompressError::Corrupt),
        }
    }
    Ok(out.buf)
}

/// Decode only the first stream in a SquashFS block; bytes after its footer are not another block.
pub(crate) fn xz_single(input: &[u8], max_out: u64) -> Result<Vec<u8>, DecompressError> {
    let mut out = CapWrite::new(cap_of(max_out));
    let mut cursor = io::Cursor::new(input);
    match lzma_rs::xz_decompress(&mut cursor, &mut out) {
        Ok(()) => Ok(out.buf),
        Err(_) if out.exceeded => Err(DecompressError::TooLarge),
        Err(lzma_rs::error::Error::HeaderTooShort(_)) => Err(DecompressError::Truncated),
        Err(_) => {
            // The decoder requires EOF after the first footer. Recheck its consumed prefix as
            // a complete stream, rather than accepting partial output from a failed decode.
            let end = usize::try_from(cursor.position()).map_err(|_| DecompressError::Corrupt)?;
            if end == 0 || end >= input.len() {
                return Err(DecompressError::Corrupt);
            }
            let prefix = input.get(..end).ok_or(DecompressError::Truncated)?;
            drop(out);
            xz(prefix, max_out)
        }
    }
}

/// Find stream boundaries backwards through their checked footer/index sizes, without
/// repeatedly trying the decoder at candidate magic bytes inside compressed data.
fn xz_stream_ranges(input: &[u8], max_out: u64) -> Result<Vec<std::ops::Range<usize>>, DecompressError> {
    let mut ranges = Vec::new();
    let mut end = input.len();
    let mut declared = 0u64;
    while end > 0 {
        let padded_end = end;
        while end > 0 && input.get(end - 1) == Some(&0) {
            end -= 1;
        }
        if end == 0 || !(padded_end - end).is_multiple_of(4) {
            return Err(DecompressError::Corrupt);
        }
        let footer_at = end.checked_sub(12).ok_or(DecompressError::Truncated)?;
        let footer = input.get(footer_at..end).ok_or(DecompressError::Truncated)?;
        let read = crate::bytes::Reader::new(footer);
        let footer_fields = footer.get(4..10).ok_or(DecompressError::Truncated)?;
        if footer.get(10..) != Some(b"YZ") || read.u32_le(0) != Some(crate::firmware::crc32(footer_fields)) {
            return Err(DecompressError::Corrupt);
        }
        let backward = u64::from(read.u32_le(4).ok_or(DecompressError::Truncated)?);
        let index_len = usize::try_from((backward + 1) * 4).map_err(|_| DecompressError::Corrupt)?;
        let index_at = footer_at.checked_sub(index_len).ok_or(DecompressError::Truncated)?;
        let index = input.get(index_at..footer_at).ok_or(DecompressError::Truncated)?;
        let crc_at = index.len().checked_sub(4).ok_or(DecompressError::Truncated)?;
        let index_read = crate::bytes::Reader::new(index);
        let index_fields = index.get(..crc_at).ok_or(DecompressError::Truncated)?;
        if index.first() != Some(&0) || index_read.u32_le(crc_at) != Some(crate::firmware::crc32(index_fields)) {
            return Err(DecompressError::Corrupt);
        }
        let mut records = index.get(1..crc_at).ok_or(DecompressError::Corrupt)?;
        let count = xz_vli(&mut records)?;
        if count > (records.len() / 2) as u64 {
            return Err(DecompressError::Corrupt);
        }
        let mut block_bytes = 0usize;
        for _ in 0..count {
            let unpadded = xz_vli(&mut records)?;
            if unpadded == 0 {
                return Err(DecompressError::Corrupt);
            }
            let padded = usize::try_from(unpadded.checked_add(3).ok_or(DecompressError::Corrupt)? & !3).map_err(|_| DecompressError::Corrupt)?;
            block_bytes = block_bytes.checked_add(padded).filter(|bytes| *bytes <= index_at).ok_or(DecompressError::Corrupt)?;
            declared = declared.checked_add(xz_vli(&mut records)?).filter(|size| *size <= max_out).ok_or(DecompressError::TooLarge)?;
        }
        if records.len() > 3 || records.iter().any(|&byte| byte != 0) {
            return Err(DecompressError::Corrupt);
        }
        let start = index_at.checked_sub(block_bytes).and_then(|at| at.checked_sub(12)).ok_or(DecompressError::Corrupt)?;
        let magic_end = start.checked_add(6).ok_or(DecompressError::Corrupt)?;
        if input.get(start..magic_end) != Some(b"\xfd7zXZ\0") {
            return Err(DecompressError::Corrupt);
        }
        ranges.push(start..end);
        end = start;
    }
    if ranges.is_empty() {
        return Err(DecompressError::Corrupt);
    }
    Ok(ranges)
}

/// XZ variable-length integers have at most nine bytes and no redundant final zero.
fn xz_vli(input: &mut &[u8]) -> Result<u64, DecompressError> {
    let mut value = 0u64;
    for shift in (0..63).step_by(7) {
        let (&byte, rest) = input.split_first().ok_or(DecompressError::Truncated)?;
        *input = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return if shift != 0 && byte == 0 { Err(DecompressError::Corrupt) } else { Ok(value) };
        }
    }
    Err(DecompressError::Corrupt)
}

/// Decompress a standalone LZMA (`.lzma`, `0x5d` props) stream, producing at most `max_out` bytes.
pub fn lzma(input: &[u8], max_out: u64) -> Result<Vec<u8>, DecompressError> {
    let cap = cap_of(max_out).min(isize::MAX as usize);
    let (header, stream) = input.split_at_checked(13).ok_or(DecompressError::Truncated)?;
    let mut header = <[u8; 13]>::try_from(header).map_err(|_| DecompressError::Truncated)?;
    let declared = u64::from_le_bytes(header[5..13].try_into().map_err(|_| DecompressError::Truncated)?);
    if declared != u64::MAX && declared > cap as u64 {
        return Err(DecompressError::TooLarge);
    }
    // The decoder retains a window before flushing to CapWrite. Bound that window too,
    // so a tiny output budget cannot first grow an arbitrarily large dictionary.
    let dictionary = u32::from_le_bytes(header[1..5].try_into().map_err(|_| DecompressError::Truncated)?);
    let window_limit = u32::try_from(cap).unwrap_or(u32::MAX).max(4096);
    let dictionary = dictionary.max(4096).min(window_limit);
    header[1..5].copy_from_slice(&dictionary.to_le_bytes());
    let options = lzma_rs::decompress::Options { memlimit: Some(dictionary as usize), ..Default::default() };
    let mut out = CapWrite::new(cap);
    let mut cursor = io::Cursor::new(header).chain(io::Cursor::new(stream));
    match lzma_rs::lzma_decompress_with_options(&mut cursor, &mut out, &options) {
        Ok(()) => Ok(out.buf),
        Err(_) if out.exceeded => Err(DecompressError::TooLarge),
        Err(lzma_rs::error::Error::HeaderTooShort(_)) => Err(DecompressError::Truncated),
        Err(_) => Err(DecompressError::Corrupt),
    }
}

/// Decode a complete LZMA piece with properties but no size header, bounding its dictionary and output.
pub(crate) fn lzma_raw_complete(input: &[u8], out_len: u32) -> Result<Vec<u8>, DecompressError> {
    let cap = usize::try_from(out_len).map_err(|_| DecompressError::TooLarge)?;
    if cap > isize::MAX as usize {
        return Err(DecompressError::TooLarge);
    }
    let (properties, stream) = input.split_at_checked(5).ok_or(DecompressError::Truncated)?;
    let [property, d0, d1, d2, d3] = <[u8; 5]>::try_from(properties).map_err(|_| DecompressError::Truncated)?;
    // No match can reach farther back than the entire output. Keep the decoder's minimum window,
    // but avoid oversized dictionary arithmetic on 32-bit targets before its memory limit is checked.
    let dictionary = u32::from_le_bytes([d0, d1, d2, d3]).max(4096).min(out_len.max(4096));
    let [d0, d1, d2, d3] = dictionary.to_le_bytes();
    let properties = [property, d0, d1, d2, d3];
    let decode = |size: Option<u64>| {
        let options =
            lzma_rs::decompress::Options { unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(size), memlimit: Some(cap), ..Default::default() };
        let mut out = CapWrite::new(cap);
        let mut cursor = io::Cursor::new(properties).chain(io::Cursor::new(stream));
        match lzma_rs::lzma_decompress_with_options(&mut cursor, &mut out, &options) {
            Ok(()) if cursor.get_ref().1.position() == stream.len() as u64 => Ok(out.buf),
            Ok(()) => Err(DecompressError::Corrupt),
            Err(_) if out.exceeded => Err(DecompressError::TooLarge),
            Err(lzma_rs::error::Error::HeaderTooShort(_)) => Err(DecompressError::Truncated),
            Err(_) => Err(DecompressError::Corrupt),
        }
    };
    let output = decode(None)?;
    if output.len() == cap {
        return Ok(output);
    }
    // A zero range code at EOF can precede the last zero literals in a stream without an end marker.
    // Retry a short result with its known size only after completion succeeded; never recover a failed probe.
    drop(output);
    decode(Some(u64::from(out_len)))
}

/// Decode the property bytes and raw stream of a ZIP method-14 member.
#[cfg(test)]
pub(crate) fn lzma_zip(input: &[u8], expected_size: u64, has_end_marker: bool, max_out: u64) -> Result<Vec<u8>, DecompressError> {
    let cap = cap_of(max_out).min(isize::MAX as usize);
    if expected_size > cap as u64 {
        return Err(DecompressError::TooLarge);
    }
    let (properties, stream) = input.split_at_checked(5).ok_or(DecompressError::Truncated)?;
    let [property, d0, d1, d2, d3] = <[u8; 5]>::try_from(properties).map_err(|_| DecompressError::Truncated)?;
    let window_limit = u32::try_from(cap).unwrap_or(u32::MAX).max(4096);
    let dictionary = u32::from_le_bytes([d0, d1, d2, d3]).max(4096).min(window_limit);
    let [d0, d1, d2, d3] = dictionary.to_le_bytes();
    let properties = [property, d0, d1, d2, d3];
    let size = if has_end_marker { None } else { Some(expected_size) };
    let options = lzma_rs::decompress::Options {
        unpacked_size: lzma_rs::decompress::UnpackedSize::UseProvided(size),
        memlimit: Some(dictionary as usize),
        ..Default::default()
    };
    let mut out = CapWrite::new(cap);
    let mut cursor = io::Cursor::new(properties).chain(io::Cursor::new(stream));
    match lzma_rs::lzma_decompress_with_options(&mut cursor, &mut out, &options) {
        Ok(()) if cursor.get_ref().1.position() == stream.len() as u64 => Ok(out.buf),
        Ok(()) => Err(DecompressError::Corrupt),
        Err(_) if out.exceeded => Err(DecompressError::TooLarge),
        Err(lzma_rs::error::Error::HeaderTooShort(_)) => Err(DecompressError::Truncated),
        Err(_) => Err(DecompressError::Corrupt),
    }
}

/// Decompress a bzip2 stream, producing at most `max_out` bytes.
pub fn bzip2(input: &[u8], max_out: u64) -> Result<Vec<u8>, DecompressError> {
    read_capped(bzip2_rs::DecoderReader::new(input), cap_of(max_out))
}

/// Decompress a Zstandard stream, producing at most `max_out` bytes.
pub fn zstd(mut input: &[u8], max_out: u64) -> Result<Vec<u8>, DecompressError> {
    if input.is_empty() {
        return Err(DecompressError::Corrupt);
    }
    let mut out = CapWrite::new(cap_of(max_out));
    while !input.is_empty() {
        if matches!(input.get(..4), Some([0x50..=0x5f, 0x2a, 0x4d, 0x18])) {
            let header = input.get(4..8).and_then(|s| s.try_into().ok()).ok_or(DecompressError::Truncated)?;
            let len = u32::from_le_bytes(header) as usize;
            input = input.get(8..).and_then(|body| body.get(len..)).ok_or(DecompressError::Truncated)?;
            continue;
        }
        let mut decoder = ruzstd::decoding::StreamingDecoder::new(&mut input).map_err(|_| DecompressError::Corrupt)?;
        if io::copy(&mut decoder, &mut out).is_err() {
            return Err(if out.exceeded { DecompressError::TooLarge } else { DecompressError::Corrupt });
        }
    }
    Ok(out.buf)
}

/// Decompress one raw LZ4 block (no frame) that inflates to exactly `out_len` bytes, as Unity
/// bundles store them.
pub fn lz4_block(input: &[u8], out_len: usize) -> Result<Vec<u8>, DecompressError> {
    let out = lz4_block_capped(input, out_len)?;
    if out.len() != out_len {
        return Err(DecompressError::Truncated);
    }
    Ok(out)
}

/// Decode one raw LZ4 block when its output size is an upper bound, as in SquashFS metadata.
pub(crate) fn lz4_block_capped(input: &[u8], out_len: usize) -> Result<Vec<u8>, DecompressError> {
    let mut out = Vec::with_capacity(out_len.min(64 << 20));
    let mut i = 0usize;
    let byte = |i: &mut usize| -> Result<u8, DecompressError> {
        let b = *input.get(*i).ok_or(DecompressError::Truncated)?;
        *i += 1;
        Ok(b)
    };
    // A 4-bit length of 15 continues in the following bytes, 255 at a time.
    let length = |i: &mut usize, base: u8| -> Result<usize, DecompressError> {
        let mut n = usize::from(base);
        if base == 15 {
            loop {
                let b = byte(i)?;
                n = n.checked_add(usize::from(b)).ok_or(DecompressError::Corrupt)?;
                if b != 255 {
                    break;
                }
            }
        }
        Ok(n)
    };
    while i < input.len() {
        let token = byte(&mut i)?;
        let literals = length(&mut i, token >> 4)?;
        let end = i.checked_add(literals).ok_or(DecompressError::Corrupt)?;
        let run = input.get(i..end).ok_or(DecompressError::Truncated)?;
        if out.len() + literals > out_len {
            return Err(DecompressError::TooLarge);
        }
        out.extend_from_slice(run);
        i = end;
        // The last sequence has literals only.
        if i >= input.len() {
            break;
        }
        let offset = usize::from(u16::from_le_bytes([byte(&mut i)?, byte(&mut i)?]));
        if offset == 0 || offset > out.len() {
            return Err(DecompressError::Corrupt);
        }
        let len = length(&mut i, token & 15)?.checked_add(4).ok_or(DecompressError::Corrupt)?;
        if out.len() + len > out_len {
            return Err(DecompressError::TooLarge);
        }
        // Each copied run extends the same repeating pattern, so it can
        // supply the next run even when the match overlaps its destination.
        let start = out.len() - offset;
        let mut remaining = len;
        while remaining > 0 {
            let count = remaining.min(out.len() - start);
            out.extend_from_within(start..start + count);
            remaining -= count;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_testkit::firmware as fx;

    #[test]
    fn lz4_blocks_with_literals_and_overlapping_matches() {
        // "abcabcabcabc!": 3 literals, a 9-byte match 3 back (overlapping), then "!".
        let block = [0x35, b'a', b'b', b'c', 3, 0, 0x10, b'!'];
        assert_eq!(lz4_block(&block, 13).unwrap(), b"abcabcabcabc!");
        // A long literal run uses the extra length bytes.
        let mut long = vec![0xf0, 5]; // 15 + 5 = 20 literals
        long.extend_from_slice(&[b'x'; 20]);
        assert_eq!(lz4_block(&long, 20).unwrap(), [b'x'; 20]);
        assert_eq!(lz4_block(&block, 12), Err(DecompressError::TooLarge));
        assert_eq!(lz4_block(&block, 14), Err(DecompressError::Truncated));
        assert_eq!(lz4_block(&[0x10, b'a', 9, 0], 10), Err(DecompressError::Corrupt), "a match before the start");
        for n in 0..64u8 {
            let junk: Vec<u8> = (0..n).map(|k| k.wrapping_mul(37).wrapping_add(n)).collect();
            let _ = lz4_block(&junk, 100);
        }
    }

    #[test]
    fn long_lz4_matches_repeat_the_entire_offset_pattern() {
        for pattern in [&b"x"[..], &b"abc"[..], &b"01234567"[..]] {
            let mut block = vec![((pattern.len() as u8) << 4) | 15];
            block.extend_from_slice(pattern);
            block.extend_from_slice(&(pattern.len() as u16).to_le_bytes());
            block.extend_from_slice(&[255, 255, 7, 0x50]);
            block.extend_from_slice(b"final");
            let mut expected = pattern.to_vec();
            expected.extend(pattern.iter().copied().cycle().take(19 + 255 + 255 + 7));
            expected.extend_from_slice(b"final");
            assert_eq!(lz4_block(&block, expected.len()).unwrap(), expected);
        }
    }

    #[test]
    fn xz_round_trip() {
        let c = fx::XZ_FIXTURE;
        assert_eq!(xz(c, 1 << 20).unwrap(), fx::XZ_PLAIN);
        // A tiny cap reports TooLarge, never a partial lie.
        assert_eq!(xz(c, 4), Err(DecompressError::TooLarge));
        // Truncation never panics.
        for len in 0..c.len() {
            let _ = xz(&c[..len], 1 << 20);
        }
    }

    #[test]
    fn xz_concatenated_streams_and_padding_share_one_budget() {
        let input = [fx::XZ_FIXTURE, &[0; 4], fx::XZ_FIXTURE, &[0; 8]].concat();
        let expected = [fx::XZ_PLAIN, fx::XZ_PLAIN].concat();
        assert_eq!(xz(&input, expected.len() as u64).unwrap(), expected);
        assert_eq!(xz(&input, expected.len() as u64 - 1), Err(DecompressError::TooLarge));
        let mut empty = Vec::new();
        lzma_rs::xz_compress(&mut io::Cursor::new(b""), &mut empty).unwrap();
        let with_empty = [&empty[..], fx::XZ_FIXTURE, &empty[..]].concat();
        assert_eq!(xz(&with_empty, fx::XZ_PLAIN.len() as u64).unwrap(), fx::XZ_PLAIN);
        assert!(xz(&empty, 0).unwrap().is_empty());
    }

    #[test]
    fn xz_first_stream_ignores_the_tail_and_keeps_its_own_budget() {
        let cap = fx::XZ_PLAIN.len() as u64;
        assert_eq!(xz_single(fx::XZ_FIXTURE, cap).unwrap(), fx::XZ_PLAIN);
        for tail in [fx::XZ_FIXTURE, &fx::XZ_FIXTURE[..13], b"junk", &[0; 4]] {
            let input = [fx::XZ_FIXTURE, tail].concat();
            assert_eq!(xz_single(&input, cap).unwrap(), fx::XZ_PLAIN);
            assert_eq!(xz_single(&input, cap - 1), Err(DecompressError::TooLarge));
        }
        let mut empty = Vec::new();
        lzma_rs::xz_compress(&mut io::Cursor::new(b""), &mut empty).unwrap();
        let input = [&empty[..], fx::XZ_FIXTURE].concat();
        assert!(xz_single(&input, 0).unwrap().is_empty());
    }

    #[test]
    fn xz_first_stream_never_returns_failed_decode_output() {
        for len in 0..fx::XZ_FIXTURE.len() {
            assert!(xz_single(&fx::XZ_FIXTURE[..len], 1024).is_err(), "truncated at {len}");
        }
        for at in [0, 8, fx::XZ_FIXTURE.len() - 13, fx::XZ_FIXTURE.len() - 12, fx::XZ_FIXTURE.len() - 1] {
            let mut input = fx::XZ_FIXTURE.to_vec();
            input[at] ^= 1;
            input.extend_from_slice(fx::XZ_FIXTURE);
            assert!(xz_single(&input, 1024).is_err(), "damaged at {at}");
        }
    }

    #[test]
    fn xz_rejects_incomplete_members_and_nonstandard_padding() {
        for padding in [1, 2, 3, 5] {
            let input = [fx::XZ_FIXTURE, &vec![0; padding]].concat();
            assert!(xz(&input, 1000).is_err());
        }
        assert!(xz(&[&[0; 4][..], fx::XZ_FIXTURE].concat(), 1000).is_err());
        assert!(xz(&[fx::XZ_FIXTURE, b"junk"].concat(), 1000).is_err());
        for missing in 1..fx::XZ_FIXTURE.len() {
            let input = [fx::XZ_FIXTURE, &fx::XZ_FIXTURE[..fx::XZ_FIXTURE.len() - missing]].concat();
            assert!(xz(&input, 1000).is_err(), "missing {missing} bytes");
        }
        for at in [fx::XZ_FIXTURE.len() - 13, fx::XZ_FIXTURE.len() - 12] {
            let mut damaged = fx::XZ_FIXTURE.to_vec();
            damaged[at] ^= 1;
            assert!(xz(&damaged, 1000).is_err());
        }
    }

    #[test]
    fn xz_index_sizes_and_integer_encodings_are_checked() {
        assert_eq!(xz_vli(&mut &[0x80, 0][..]), Err(DecompressError::Corrupt));
        assert_eq!(xz_vli(&mut &[0xff; 10][..]), Err(DecompressError::Corrupt));
        assert_eq!(xz_vli(&mut &[0x81, 1][..]), Ok(129));
        let mut damaged = fx::XZ_FIXTURE.to_vec();
        let footer = damaged.len() - 12;
        let backward = u32::from_le_bytes(damaged[footer + 4..footer + 8].try_into().unwrap());
        let index = footer - (backward as usize + 1) * 4;
        damaged[index + 2] = 0;
        let checksum = crate::firmware::crc32(&damaged[index..footer - 4]);
        damaged[footer - 4..footer].copy_from_slice(&checksum.to_le_bytes());
        assert!(xz(&damaged, 1000).is_err());
    }

    #[test]
    fn lzma_round_trip() {
        let c = fx::LZMA_FIXTURE;
        assert_eq!(lzma(c, 1 << 20).unwrap(), fx::LZMA_PLAIN);
        assert_eq!(lzma(c, 4), Err(DecompressError::TooLarge));
        for len in 0..c.len() {
            let _ = lzma(&c[..len], 1 << 20);
        }
    }

    #[test]
    fn standalone_lzma_checks_declared_sizes_before_decoding() {
        let mut header = [0u8; 13];
        header[0] = 0x5d;
        header[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
        header[5..13].copy_from_slice(&1_000_000_000u64.to_le_bytes());
        assert_eq!(lzma(&header, 64), Err(DecompressError::TooLarge));
        assert_eq!(lzma(&header[..12], 64), Err(DecompressError::Truncated));
        let mut valid = fx::LZMA_FIXTURE.to_vec();
        valid[1..5].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(lzma(&valid, fx::LZMA_PLAIN.len() as u64).unwrap(), fx::LZMA_PLAIN);
        assert_eq!(lzma(&valid, 0), Err(DecompressError::TooLarge));
    }

    #[test]
    fn standalone_lzma_stops_before_a_large_window_reaches_a_damaged_tail() {
        let plain = vec![b'a'; 16 * 1024];
        let mut compressed = Vec::new();
        lzma_rs::lzma_compress(&mut io::Cursor::new(&plain), &mut compressed).unwrap();
        compressed.truncate(compressed.len() - 5);
        assert_eq!(lzma(&compressed, 64), Err(DecompressError::TooLarge));
        assert_eq!(lzma(&compressed, plain.len() as u64), Err(DecompressError::Corrupt));
    }

    #[test]
    fn bzip2_round_trip() {
        let c = fx::BZIP2_FIXTURE;
        assert_eq!(bzip2(c, 1 << 20).unwrap(), fx::BZIP2_PLAIN);
        assert_eq!(bzip2(c, 2), Err(DecompressError::TooLarge));
        for len in 0..c.len() {
            let _ = bzip2(&c[..len], 1 << 20);
        }
    }

    #[test]
    fn zstd_round_trip() {
        let c = fx::ZSTD_FIXTURE;
        assert_eq!(zstd(c, 1 << 20).unwrap(), fx::ZSTD_PLAIN);
        assert_eq!(zstd(c, 2), Err(DecompressError::TooLarge));
        for len in 0..c.len() {
            let _ = zstd(&c[..len], 1 << 20);
        }
    }

    #[test]
    fn zstandard_members_can_contain_multiple_frames() {
        let frame = fx::ZSTD_FIXTURE;
        let plain = fx::ZSTD_PLAIN;
        let skip = [0x50, 0x2a, 0x4d, 0x18, 3, 0, 0, 0, 1, 2, 3];
        let input = [frame, &skip, frame].concat();
        let expected = [plain, plain].concat();
        assert_eq!(zstd(&input, expected.len() as u64).unwrap(), expected);
        assert_eq!(zstd(&input, plain.len() as u64), Err(DecompressError::TooLarge));
        assert!(zstd(&[frame, &skip[..9]].concat(), u64::MAX).is_err());
        assert!(zstd(&[frame, &frame[..5]].concat(), u64::MAX).is_err());
    }

    #[test]
    fn garbage_never_panics() {
        for seed in 0..64u64 {
            let n = craft_anything_testkit::noise(seed, 400);
            let _ = xz(&n, 1 << 20);
            let _ = lzma(&n, 1 << 20);
            let _ = bzip2(&n, 1 << 20);
            let _ = zstd(&n, 1 << 20);
        }
    }
}
