//! Bounded seekable ZIP entry reading from craft-zip.
pub use craft_zip::lenient::file::read_named_entry;

#[cfg(test)]
#[path = "zip_file_legacy_tests.rs"]
mod legacy;

#[cfg(test)]
mod parity_tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn seekable_results_match_legacy_with_damage_names_and_limits() {
        let bytes = craft_anything_testkit::electron::zip_stored(&[("META-INF/MANIFEST.MF", b"Manifest-Version: 1.0\n"), ("empty", b"")]);
        for data in std::iter::once(bytes.clone()).chain(craft_anything_testkit::mutations(&bytes)) {
            for name in ["META-INF/MANIFEST.MF", "empty", "absent"] {
                for cap in [0, 2, 4096] {
                    let actual = read_named_entry(&mut Cursor::new(&data), name, cap).map_err(|e| e.to_string());
                    let expected = legacy::read_named_entry(&mut Cursor::new(&data), name, cap).map_err(|e| e.to_string());
                    assert_eq!(actual, expected);
                }
            }
        }
    }
}
