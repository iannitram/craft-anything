//! Unity AudioClip objects and the FMOD sound banks (FSB5) their sound is stored in.
//!
//! An AudioClip from Unity 5 on holds no sound itself. After its name come the load type, the
//! channel count, the frequency and bits per sample (4 bytes each), the length in seconds (a
//! float), a tracker-format flag (and from 2017 an ambisonic one in the same 4 bytes); the subsound
//! index, three flags (preload, load in background, legacy 3D), aligned; then where the sound is
//! (a file path, a 64-bit offset and size: a `.resource` file beside the serialized file, or
//! `archive:/…` inside a bundle) and the compression format.
//!
//! The bytes there are an FSB5 bank: `FSB5`, its version, the sample count, the sizes of the
//! sample headers, the name table and the data, and the codec, then 32 more header bytes (36 in
//! version 0). Each sample header is 64 bits: bit 0 says chunks follow, bits 1-4 are a frequency
//! index, bit 5 says stereo, bits 6-33 the data offset in 16-byte units and bits 34-63 the frame
//! count; the chunks after it can set the channel count or the frequency. Readings of the format
//! differ on bit 6 (a channel-count bit, the offset then in 32-byte units), so a header with it set
//! is refused rather than read one way. Sample data follows the name table, each sample's up to
//! the next one's.

use crate::common::FormatError;
use crate::unity_serialized::{Cur, version_numbers};
use serde::Serialize;

/// The oldest Unity version whose AudioClip layout is read.
pub const MIN_VERSION: (u32, u32) = (5, 0);
/// The longest name kept.
const MAX_NAME: usize = 1024;
/// The most samples a bank may hold (Unity writes one).
pub const MAX_SAMPLES: usize = 65_536;

/// Where a clip's sound is stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StreamRef {
    pub path: String,
    pub offset: u64,
    pub size: u64,
}

/// An AudioClip's description.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Clip {
    pub name: String,
    pub channels: i32,
    pub frequency: i32,
    pub bits_per_sample: i32,
    /// Seconds.
    pub length: f32,
    pub subsound_index: i32,
    /// Unity's compression format id ([`compression_name`]).
    pub compression: i32,
    pub stream: StreamRef,
}

fn string(c: &mut Cur, what: &str) -> Result<String, FormatError> {
    let n = c.count(what, 1)?;
    let s = String::from_utf8_lossy(c.take(n, what)?).chars().take(MAX_NAME).collect();
    c.align4()?;
    Ok(s)
}

/// Read an AudioClip object's bytes (`object`), from a file written by `unity_version`.
pub fn parse(object: &[u8], unity_version: &str, big_endian: bool) -> Result<Clip, FormatError> {
    let v = version_numbers(unity_version).ok_or_else(|| FormatError::Malformed(format!("Unity version {unity_version:?}")))?;
    if v < MIN_VERSION {
        return Err(FormatError::Unsupported(format!("sounds from Unity {unity_version} (before 5.0)")));
    }
    let unknown = || FormatError::Unsupported(format!("this sound's layout (Unity {unity_version}) isn't one that's read here"));
    let mut c = Cur { data: object, pos: 0, big: big_endian };
    let name = string(&mut c, "name")?;
    c.i32("load type")?;
    let channels = c.i32("channels")?;
    let frequency = c.i32("frequency")?;
    let bits_per_sample = c.i32("bits per sample")?;
    let length = f32::from_bits(c.u32("length")?);
    c.u8("tracker format")?;
    c.align4()?;
    let subsound_index = c.i32("subsound index")?;
    c.take(3, "load flags")?;
    c.align4()?;
    let path = string(&mut c, "resource path")?;
    let offset = c.u64("resource offset")?;
    let size = c.u64("resource size")?;
    let compression = c.i32("compression format")?;
    // What a wrong layout would give: an impossible count, rate or length, or bytes left over.
    if !(1..=64).contains(&channels)
        || !(1..=768_000).contains(&frequency)
        || !(0..=64).contains(&bits_per_sample)
        || !length.is_finite()
        || length < 0.0
        || c.pos != object.len()
    {
        return Err(unknown());
    }
    if path.is_empty() || size == 0 {
        return Err(FormatError::Unsupported(format!("{name} keeps no sound of its own (it names no .resource file, or no bytes in it)")));
    }
    Ok(Clip { name, channels, frequency, bits_per_sample, length, subsound_index, compression, stream: StreamRef { path, offset, size } })
}

/// Unity's audio compression formats by id.
const COMPRESSIONS: &[&str] = &["PCM", "Vorbis", "ADPCM", "MP3", "VAG", "HEVAG", "XMA", "AAC", "GCADPCM", "ATRAC9"];

/// A compression format's name, or `format <id>`.
pub fn compression_name(id: i32) -> String {
    usize::try_from(id).ok().and_then(|i| COMPRESSIONS.get(i)).map_or_else(|| format!("format {id}"), |n| (*n).to_string())
}

/// The FSB5 magic.
pub const FSB5: &[u8; 4] = b"FSB5";

/// FSB5 codecs by id.
const CODECS: &[&str] =
    &["none", "PCM8", "PCM16", "PCM24", "PCM32", "PCMFLOAT", "GCADPCM", "IMAADPCM", "VAG", "HEVAG", "XMA", "MPEG", "CELT", "AT9", "XWMA", "Vorbis"];

/// An FSB5 codec's name, or `codec <id>`.
pub fn codec_name(id: u32) -> String {
    usize::try_from(id).ok().and_then(|i| CODECS.get(i)).map_or_else(|| format!("codec {id}"), |n| (*n).to_string())
}

/// Sample rates by the index in a sample header.
const RATES: [u32; 10] = [0, 8000, 11000, 11025, 16000, 22050, 24000, 32000, 44100, 48000];

/// One sound in a bank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Sample {
    pub frequency: u32,
    pub channels: u32,
    pub frames: u32,
    /// Where its data is in the bank's bytes: start and length.
    #[serde(skip)]
    pub data: (usize, usize),
}

/// An FSB5 bank's header and samples.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Bank {
    pub version: u32,
    pub codec: u32,
    pub samples: Vec<Sample>,
    /// Bytes the bank takes, from `FSB5` to the end of its data.
    pub size: usize,
}

fn le32(data: &[u8], at: usize, what: &str) -> Result<u32, FormatError> {
    let b = data.get(at..at.checked_add(4).ok_or_else(|| FormatError::Truncated(what.into()))?).ok_or_else(|| FormatError::Truncated(what.into()))?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn bits(value: u64, start: u32, len: u32) -> u64 {
    (value >> start) & ((1 << len) - 1)
}

/// Read an FSB5 bank at the start of `data`.
pub fn bank(data: &[u8]) -> Result<Bank, FormatError> {
    if !data.starts_with(FSB5) {
        return Err(FormatError::Malformed("not an FSB5 bank".into()));
    }
    let version = le32(data, 4, "FSB5 version")?;
    let header = match version {
        0 => 0x40,
        1 => 0x3c,
        v => return Err(FormatError::Unsupported(format!("FSB5 version {v}"))),
    };
    let field = |i: usize, what: &str| le32(data, 8 + 4 * i, what).map(|n| n as usize);
    let (count, headers, names, size) = (field(0, "sample count")?, field(1, "sample headers size")?, field(2, "name table size")?, field(3, "data size")?);
    let codec = le32(data, 24, "codec")?;
    let truncated = || FormatError::Truncated("FSB5 sample data".into());
    let table_end = headers.checked_add(header).ok_or_else(truncated)?;
    let start = table_end.checked_add(names).ok_or_else(truncated)?;
    let end = start.checked_add(size).filter(|e| *e <= data.len()).ok_or_else(truncated)?;
    // Each sample header takes 8 bytes at least, so a count that can't fit is refused unread.
    if count == 0 || count > MAX_SAMPLES || count.saturating_mul(8) > headers {
        return Err(FormatError::Malformed(format!("FSB5 sample count {count} doesn't fit its {headers}-byte headers")));
    }
    let table = data.get(header..table_end).ok_or_else(|| FormatError::Truncated("FSB5 sample headers".into()))?;
    let mut at = 0;
    let mut found = Vec::with_capacity(count);
    for _ in 0..count {
        let raw = table.get(at..at + 8).ok_or_else(|| FormatError::Truncated("FSB5 sample header".into()))?;
        let raw = u64::from_le_bytes(raw.try_into().map_err(|_| FormatError::Truncated("FSB5 sample header".into()))?);
        at += 8;
        if raw & 1 << 6 != 0 {
            return Err(FormatError::Unsupported("an FSB5 sample header with bit 6 set, which readings of the format disagree on".into()));
        }
        let mut frequency = RATES.get(bits(raw, 1, 4) as usize).copied().unwrap_or(0);
        let mut channels = bits(raw, 5, 1) as u32 + 1;
        let offset = bits(raw, 7, 27) as usize * 32;
        let frames = bits(raw, 34, 30) as u32;
        let mut more = raw & 1 == 1;
        while more {
            let chunk = le32(table, at, "FSB5 sample chunk")?;
            at += 4;
            more = chunk & 1 == 1;
            let len = ((chunk >> 1) & 0xff_ffff) as usize;
            let body = table
                .get(at..at.checked_add(len).ok_or_else(|| FormatError::Truncated("FSB5 sample chunk".into()))?)
                .ok_or_else(|| FormatError::Truncated("FSB5 sample chunk".into()))?;
            match (chunk >> 25, body) {
                (1, [n, ..]) => channels = u32::from(*n),
                (2, [a, b, c, d, ..]) => frequency = u32::from_le_bytes([*a, *b, *c, *d]),
                _ => {}
            }
            at += len;
        }
        if offset > size {
            return Err(FormatError::Malformed(format!("an FSB5 sample starts past its data ({offset} of {size} bytes)")));
        }
        found.push((Sample { frequency, channels, frames, data: (start.checked_add(offset).ok_or_else(truncated)?, 0) }, offset));
    }
    let mut samples = Vec::with_capacity(count);
    for i in 0..found.len() {
        let next = found.get(i + 1).map_or(size, |(_, o)| *o);
        let (mut sample, offset) = found[i].clone();
        let len = next.checked_sub(offset).ok_or_else(|| FormatError::Malformed("FSB5 samples are out of order".into()))?;
        sample.data.1 = len;
        samples.push(sample);
    }
    Ok(Bank { version, codec, samples, size: end })
}

/// A WAV file of one sample of `bank` (whose bytes are `data`), or `None` unless it's 16-bit PCM in
/// mono or stereo (what Unity's PCM is, and what a plain WAV header describes) whose frames fit.
pub fn wav(bank: &Bank, sample: &Sample, data: &[u8]) -> Option<Vec<u8>> {
    let (bytes, tag) = (2u32, 1u16);
    if bank.codec != 2 || !(1..=2).contains(&sample.channels) || sample.frequency == 0 {
        return None;
    }
    let len = u64::from(sample.frames) * u64::from(sample.channels) * u64::from(bytes);
    let len = usize::try_from(len).ok().filter(|n| *n <= sample.data.1)?;
    let pcm = data.get(sample.data.0..sample.data.0.checked_add(len)?)?;
    let riff = u32::try_from(36 + len).ok()?;
    let block = u16::try_from(sample.channels * bytes).ok()?;
    let mut out = Vec::with_capacity(44 + len);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&riff.to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&u16::try_from(sample.channels).ok()?.to_le_bytes());
    out.extend_from_slice(&sample.frequency.to_le_bytes());
    out.extend_from_slice(&(sample.frequency.checked_mul(u32::from(block))?).to_le_bytes());
    out.extend_from_slice(&block.to_le_bytes());
    out.extend_from_slice(&(bytes as u16 * 8).to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(len as u32).to_le_bytes());
    out.extend_from_slice(pcm);
    Some(out)
}
