use craft_anything_formats::{packed::PackedFormat, pyinstaller::PyInstaller};
use craft_anything_testkit::packed::pyinstaller;

/// A damaged TOC name must not turn padding into an extractable file identity.
#[test]
fn unterminated_carchive_name_stops_the_listing() {
    for new_cookie in [false, true] {
        let mut data = pyinstaller(&[("ok", b'x', b"one", false), ("bad", b'x', b"two", false)], new_cookie, 312);
        let cookie = data.len() - if new_cookie { 88 } else { 24 };
        let number = |at| u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
        let start = data.len() - number(cookie + 8);
        let table = start + number(cookie + 12);
        let second = table + number(table);
        let end = second + number(second);
        let valid = PyInstaller.list(&data).unwrap();
        assert_eq!(valid.entries.len(), 2);
        data[second + 18..end].fill(b'x');
        let listing = PyInstaller.list(&data).unwrap();
        assert!(listing.truncated);
        assert_eq!(listing.entries.len(), 1);
        assert_eq!(listing.entries[0].path, "ok");
        assert_eq!(PyInstaller.read(&data, &listing.entries[0], 10).unwrap(), b"one");
    }
}
