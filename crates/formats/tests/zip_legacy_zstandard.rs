use craft_anything_formats::{firmware, zip};
use craft_anything_testkit::electron::zip_stored;

const BODY: &[u8] = b"zstandard payload";

/// Build a ZIP with one single-segment Zstandard frame containing a raw final block.
fn sample(method: u16) -> Vec<u8> {
    let size = BODY.len() as u8;
    let mut frame = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, size, (size << 3) | 1, 0, 0];
    frame.extend_from_slice(BODY);
    let mut data = zip_stored(&[("a.txt", &frame)]);
    let cd = 30 + "a.txt".len() + frame.len();
    data[8..10].copy_from_slice(&method.to_le_bytes());
    data[14..18].copy_from_slice(&zip::crc32(BODY).to_le_bytes());
    data[22..26].copy_from_slice(&u32::from(size).to_le_bytes());
    data[cd + 10..cd + 12].copy_from_slice(&method.to_le_bytes());
    data[cd + 16..cd + 20].copy_from_slice(&zip::crc32(BODY).to_le_bytes());
    data[cd + 24..cd + 28].copy_from_slice(&u32::from(size).to_le_bytes());
    data
}

/// The deprecated and current method IDs both decode through the bounded ZIP reader.
#[test]
fn reads_zstandard_members_under_both_method_ids() {
    for method in [93, 20] {
        let data = sample(method);
        let entry = zip::parse(&data, 1).unwrap().entries.remove(0);
        assert_eq!(entry.method_name(), "zstd", "method {method}");
        assert_eq!(zip::read_entry(&data, &entry, 100).unwrap(), BODY);
        assert!(matches!(zip::read_entry(&data, &entry, 2), Err(zip::ZipError::TooLarge(2))));
    }
}

/// The firmware scan also sees ZIPs whose first member uses the old method ID.
#[test]
fn discovers_a_zip_with_a_legacy_zstandard_first_member() {
    let data = sample(20);
    let report = firmware::scan(&data, &firmware::ScanOptions::default(), &|| false);
    assert!(report.regions.iter().any(|r| r.offset == 0 && r.kind == "zip" && r.length == Some(data.len() as u64)));
}
