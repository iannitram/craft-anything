//! Bounded TAR/CPIO member walkers from craft-tar. Extraction and scanner policy stay in the app.
pub(crate) use craft_tar::{CpioChecksum, cpio_with_checksum};
pub use craft_tar::{Listing, Member, MemberKind, StopReason, cpio, member_data, tar, tar_header_ok};

// Keep the original implementation and tests solely as an oracle during the cutover.
#[cfg(test)]
#[path = "tarcpio_legacy_tests.rs"]
mod legacy;

#[cfg(test)]
mod parity_tests {
    use super::*;
    use craft_anything_testkit::firmware as fx;

    #[test]
    fn shared_listings_and_serialization_match_legacy_on_damage_and_limits() {
        let samples = [fx::tar(&[("a", b"hello"), ("b", b"world")]), fx::cpio_newc(&[("file", b"data", 0o100644)])];
        for sample in samples {
            for bytes in std::iter::once(sample.clone()).chain(craft_anything_testkit::mutations(&sample)) {
                for max in [0, 1, 2, 10] {
                    assert_eq!(serde_json::to_value(tar(&bytes, max)).unwrap(), serde_json::to_value(legacy::tar(&bytes, max)).unwrap());
                    assert_eq!(serde_json::to_value(cpio(&bytes, max)).unwrap(), serde_json::to_value(legacy::cpio(&bytes, max)).unwrap());
                }
            }
        }
    }

    #[test]
    fn checksum_callback_skipping_and_cancellation_match_legacy() {
        let mut bytes = fx::cpio_newc(&[("file", b"data", 0o100644)]);
        bytes[..6].copy_from_slice(b"070702");
        for cancel in [false, true] {
            let mut actual_calls = 0;
            let actual = cpio_with_checksum(&bytes, 10, &mut |_| {
                actual_calls += 1;
                if cancel { CpioChecksum::Cancelled } else { CpioChecksum::Skipped }
            });
            let mut expected_calls = 0;
            let expected = legacy::cpio_with_checksum(&bytes, 10, &mut |_| {
                expected_calls += 1;
                if cancel { legacy::CpioChecksum::Cancelled } else { legacy::CpioChecksum::Skipped }
            });
            assert_eq!(actual_calls, expected_calls);
            assert_eq!(actual_calls, 1);
            assert_eq!(serde_json::to_value(actual).unwrap(), serde_json::to_value(expected).unwrap());
        }
    }
}
