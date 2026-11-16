use craft_anything_formats::tarcpio::{member_data, tar};
use craft_anything_testkit::firmware::tar_entries;

/// Encode the decimal byte count that includes the PAX record itself.
fn record(value: &str) -> String {
    let body = format!(" size={value}\n");
    let mut size = body.len() + 1;
    loop {
        let result = format!("{size}{body}");
        if result.len() == size {
            return result;
        }
        size = result.len();
    }
}

/// Invalid local sizes cannot silently fall back to a plausible member header.
#[test]
fn malformed_local_sizes_stop_before_the_affected_member() {
    for value in ["no", "-1", "+3", " 3", "3 ", "3.0", "18446744073709551616"] {
        let pax = record(value);
        let bytes = tar_entries(&[("keep", b'0', b"ok", ""), ("PaxHeader", b'x', pax.as_bytes(), ""), ("affected", b'0', b"abc", "")]);
        let report = tar(&bytes, 10).unwrap();
        assert!(!report.complete, "size={value}");
        assert_eq!(report.members.len(), 1);
        assert_eq!(report.members[0].name, "keep");
        assert_eq!(member_data(&bytes, &report.members[0]), Some(b"ok".as_slice()));
        assert!(report.warnings.iter().any(|w| w.contains("pax size")));
    }
}

/// Decimal sizes, zero, and the existing empty override behavior still work.
#[test]
fn valid_local_sizes_remain_readable() {
    for (size, body) in [("3", b"abc".as_slice()), ("0003", b"abc"), ("0", b""), ("", b"abc")] {
        let pax = record(size);
        let bytes = tar_entries(&[("PaxHeader", b'x', pax.as_bytes(), ""), ("file", b'0', body, "")]);
        let report = tar(&bytes, 10).unwrap();
        assert!(report.complete, "{:?}", report.warnings);
        assert_eq!(member_data(&bytes, &report.members[0]), Some(body));
    }
}
