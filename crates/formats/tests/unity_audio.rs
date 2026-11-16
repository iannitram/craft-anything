use craft_anything_formats::unity_audio::{self, Clip, StreamRef};
use craft_anything_testkit::games::{fsb5_bank, unity_audioclip};

#[test]
fn reads_audioclips_from_unity_5_to_unity_6() {
    for version in ["5.6.7f1", "2017.4.40f1", "2019.4.19f1", "2022.3.43f1", "6000.0.23f1"] {
        let bytes = unity_audioclip(version, "Jump", 2, 44100, 1.5, 1, ("sharedassets0.resource", 4096, 777));
        let clip = unity_audio::parse(&bytes, version, false).unwrap_or_else(|e| panic!("{version}: {e}"));
        assert_eq!(
            clip,
            Clip {
                name: "Jump".into(),
                channels: 2,
                frequency: 44100,
                bits_per_sample: 16,
                length: 1.5,
                subsound_index: 0,
                compression: 1,
                stream: StreamRef { path: "sharedassets0.resource".into(), offset: 4096, size: 777 },
            },
            "{version}"
        );
        assert_eq!(unity_audio::compression_name(clip.compression), "Vorbis");
    }
}

#[test]
fn a_layout_it_does_not_know_is_refused() {
    let bytes = unity_audioclip("2019.4.19f1", "Jump", 2, 44100, 1.5, 0, ("a.resource", 0, 10));
    // Bytes left over, or missing, mean the fields weren't where they were looked for.
    let mut longer = bytes.clone();
    longer.extend_from_slice(&[0; 4]);
    assert!(unity_audio::parse(&longer, "2019.4.19f1", false).is_err());
    assert!(unity_audio::parse(&bytes[..bytes.len() - 1], "2019.4.19f1", false).is_err());
    assert!(unity_audio::parse(&bytes, "4.7.2f1", false).is_err(), "before 5.0");
    let silent = unity_audioclip("2019.4.19f1", "Gap", 1, 22050, 0.0, 0, ("", 0, 0));
    let e = unity_audio::parse(&silent, "2019.4.19f1", false).unwrap_err().to_string();
    assert!(e.contains("no sound of its own"), "{e}");
}

#[test]
fn a_pcm_bank_becomes_a_wav() {
    let pcm: Vec<u8> = (0..20u8).collect(); // 5 stereo frames of 16 bits
    let bank_bytes = fsb5_bank(2, &[(9, 2, 5, &pcm)]);
    let bank = unity_audio::bank(&bank_bytes).unwrap();
    assert_eq!((bank.version, unity_audio::codec_name(bank.codec).as_str(), bank.size), (1, "PCM16", bank_bytes.len()));
    let [sample] = bank.samples.as_slice() else { panic!("{:?}", bank.samples) };
    assert_eq!((sample.frequency, sample.channels, sample.frames), (48000, 2, 5));
    let wav = unity_audio::wav(&bank, sample, &bank_bytes).unwrap();
    assert_eq!(&wav[..4], b"RIFF");
    assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()) as usize, wav.len() - 8);
    assert_eq!(&wav[8..16], b"WAVEfmt ");
    let field = |at: usize, n: usize| wav[at..at + n].iter().rev().fold(0u32, |v, b| v << 8 | u32::from(*b));
    // PCM, 2 channels, 48000 Hz, 192000 bytes a second, 4 a frame, 16 bits.
    assert_eq!([field(20, 2), field(22, 2), field(24, 4), field(28, 4), field(32, 2), field(34, 2)], [1, 2, 48000, 192000, 4, 16]);
    assert_eq!((&wav[36..40], field(40, 4)), (b"data".as_slice(), 20));
    assert_eq!(&wav[44..], pcm.as_slice(), "the frames, without the bank's padding");
}

#[test]
fn compressed_or_odd_banks_are_not_made_into_wavs() {
    let vorbis = fsb5_bank(15, &[(8, 1, 4410, &[7; 40])]);
    let bank = unity_audio::bank(&vorbis).unwrap();
    assert_eq!(unity_audio::codec_name(bank.codec), "Vorbis");
    assert_eq!(unity_audio::wav(&bank, &bank.samples[0], &vorbis), None);
    // A channel count past stereo is in a chunk; two samples each get their own data.
    let surround = fsb5_bank(2, &[(8, 6, 2, &[1; 24]), (5, 1, 3, &[2; 6])]);
    let bank = unity_audio::bank(&surround).unwrap();
    assert_eq!(bank.samples.iter().map(|s| (s.frequency, s.channels, s.frames, s.data.1)).collect::<Vec<_>>(), [(44100, 6, 2, 32), (22050, 1, 3, 32)]);
    assert_eq!(unity_audio::wav(&bank, &bank.samples[1], &surround).unwrap()[44..], [2; 6]);
    // Only 16-bit PCM in mono or stereo: a surround or 24-bit sample stays in its bank.
    assert_eq!(unity_audio::wav(&bank, &bank.samples[0], &surround), None);
    let pcm24 = fsb5_bank(3, &[(8, 1, 2, &[0; 6])]);
    let bank = unity_audio::bank(&pcm24).unwrap();
    assert_eq!(unity_audio::wav(&bank, &bank.samples[0], &pcm24), None);
    // A frequency chunk sets the rate the index doesn't have.
    let mut odd_rate = fsb5_bank(2, &[(0, 1, 2, &[0; 4])]);
    let raw = u64::from_le_bytes(odd_rate[0x3c..0x44].try_into().unwrap()) | 1;
    odd_rate[0x3c..0x44].copy_from_slice(&raw.to_le_bytes());
    let mut chunk = (4u32 << 1 | 2 << 25).to_le_bytes().to_vec();
    chunk.extend_from_slice(&96_000u32.to_le_bytes());
    odd_rate.splice(0x44..0x44, chunk);
    odd_rate[12..16].copy_from_slice(&16u32.to_le_bytes()); // the sample headers grew by 8 bytes
    let bank = unity_audio::bank(&odd_rate).unwrap();
    assert_eq!(bank.samples[0].frequency, 96_000);
    assert_eq!(unity_audio::wav(&bank, &bank.samples[0], &odd_rate).unwrap()[24..28], 96_000u32.to_le_bytes());
    // More frames than the sample holds: no WAV rather than one cut short.
    let short = fsb5_bank(2, &[(8, 1, 100, &[0; 8])]);
    let bank = unity_audio::bank(&short).unwrap();
    assert_eq!(unity_audio::wav(&bank, &bank.samples[0], &short), None);
}

#[test]
fn broken_banks_are_refused() {
    let good = fsb5_bank(2, &[(8, 1, 4, &[0; 8])]);
    let mut cases = vec![good[..good.len() - 1].to_vec(), good[..20].to_vec(), b"FSB4".to_vec()];
    let mut version = good.clone();
    version[4] = 7;
    cases.push(version);
    let mut count = good.clone();
    count[8..12].copy_from_slice(&1_000_000u32.to_le_bytes());
    cases.push(count);
    let mut past = good.clone();
    // The sample's data offset (in 16-byte units) past the 32 bytes of data.
    let raw = u64::from_le_bytes(past[0x3c..0x44].try_into().unwrap()) | 3 << 6;
    past[0x3c..0x44].copy_from_slice(&raw.to_le_bytes());
    cases.push(past);
    // Bit 6, which readings of the format disagree on.
    let mut bit6 = good.clone();
    let raw = u64::from_le_bytes(bit6[0x3c..0x44].try_into().unwrap()) | 1 << 6;
    bit6[0x3c..0x44].copy_from_slice(&raw.to_le_bytes());
    cases.push(bit6);
    // More samples than a bank may hold, even when their headers fit.
    let many = unity_audio::MAX_SAMPLES as u32 + 1;
    let mut crowded = b"FSB5".to_vec();
    for x in [1, many, many * 8, 0, 0, 2] {
        crowded.extend_from_slice(&x.to_le_bytes());
    }
    crowded.resize(0x3c + many as usize * 8, 0);
    cases.push(crowded);
    for bytes in cases {
        assert!(unity_audio::bank(&bytes).is_err(), "{bytes:?}");
    }
}
