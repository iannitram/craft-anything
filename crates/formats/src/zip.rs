//! Lenient ZIP inspection from craft-zip, preserving bounded reads and CRC validation.
pub(crate) use craft_zip::lenient::has_entry;
pub use craft_zip::lenient::{ZipArchive, ZipEntry, ZipError, crc32, looks_like_zip, parse, raw_entry_data, read_entry, read_entry_unverified};

#[cfg(test)]
#[path = "zip_legacy_tests.rs"]
pub(crate) mod legacy;

#[cfg(test)]
mod parity_tests {
    use super::*;

    #[test]
    fn shared_zip_matches_legacy_listing_read_errors_and_caps() {
        let bytes = craft_anything_testkit::electron::zip_stored(&[("a.txt", b"abc"), ("nested/b.bin", &[0, 255, 1])]);
        for data in std::iter::once(bytes.clone()).chain(craft_anything_testkit::mutations(&bytes)) {
            assert_eq!(looks_like_zip(&data), legacy::looks_like_zip(&data));
            assert_eq!(has_entry(&data, "a.txt"), legacy::has_entry(&data, "a.txt"));
            for max in [0, 1, 10] {
                let actual = parse(&data, max);
                let old = legacy::parse(&data, max);
                assert_eq!(
                    actual.as_ref().map(|a| serde_json::to_value(a).unwrap()).map_err(ToString::to_string),
                    old.as_ref().map(|a| serde_json::to_value(a).unwrap()).map_err(ToString::to_string)
                );
                if let (Ok(actual), Ok(old)) = (actual, old) {
                    for (a, b) in actual.entries.iter().zip(&old.entries) {
                        assert_eq!(a.special_kind(), b.special_kind());
                        assert_eq!(a.method_name(), b.method_name());
                        assert_eq!(raw_entry_data(&data, a).map_err(|e| e.to_string()), legacy::raw_entry_data(&data, b).map_err(|e| e.to_string()));
                        for cap in [0, 2, 4096] {
                            assert_eq!(read_entry(&data, a, cap).map_err(|e| e.to_string()), legacy::read_entry(&data, b, cap).map_err(|e| e.to_string()));
                            assert_eq!(
                                read_entry_unverified(&data, a, cap).map_err(|e| e.to_string()),
                                legacy::read_entry_unverified(&data, b, cap).map_err(|e| e.to_string())
                            );
                        }
                    }
                }
            }
        }
    }
}
