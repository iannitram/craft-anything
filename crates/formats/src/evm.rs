//! Shared read-only EVM inspection; file loading and evidence policy remain local.
pub use craft_evm::{Encoding, EvmFunction, EvmInterface, EvmMetadata, MAX_CODE, mnemonic};
impl From<craft_evm::EvmError> for crate::FormatError {
    fn from(error: craft_evm::EvmError) -> Self {
        match error {
            craft_evm::EvmError::Truncated(s) => Self::Truncated(s),
            craft_evm::EvmError::Malformed(s) => Self::Malformed(s),
            craft_evm::EvmError::Unsupported(s) => Self::Unsupported(s),
            craft_evm::EvmError::TooLarge(s) => Self::TooLarge(s),
        }
    }
}
pub fn decode_input(input: &[u8], enc: Encoding) -> Result<(Vec<u8>, &'static str), crate::FormatError> {
    craft_evm::decode_input(input, enc).map_err(Into::into)
}
pub fn analyze(code: &[u8]) -> Result<EvmInterface, crate::FormatError> {
    craft_evm::analyze(code).map_err(Into::into)
}
#[cfg(test)]
#[path = "evm_legacy.rs"]
mod legacy;
#[cfg(test)]
mod parity {
    use super::*;
    #[test]
    fn original_reports_errors_hex_and_truncations_match() {
        let (code, _) = craft_anything_testkit::formats::evm_runtime();
        let mut cases = craft_anything_testkit::mutations(&code);
        cases.extend((0..code.len()).map(|n| code[..n].to_vec()));
        cases.extend([vec![], vec![0xef, 0, 1], vec![0x7f; 40]]);
        for bytes in cases {
            assert_eq!(legacy::analyze(&bytes).map(|r| serde_json::to_value(r).unwrap()), analyze(&bytes).map(|r| serde_json::to_value(r).unwrap()));
            for (old, new) in [(legacy::Encoding::Auto, Encoding::Auto), (legacy::Encoding::Raw, Encoding::Raw), (legacy::Encoding::Hex, Encoding::Hex)] {
                assert_eq!(legacy::decode_input(&bytes, old), decode_input(&bytes, new));
            }
        }
        for bytes in [b"0x123".as_slice(), b"0xzz", b" 0x0012 ", b"0Xff", b""] {
            assert_eq!(legacy::decode_input(bytes, legacy::Encoding::Auto), decode_input(bytes, Encoding::Auto));
        }
        for op in 0..=255 {
            assert_eq!(legacy::mnemonic(op), mnemonic(op));
        }
    }
}

#[cfg(test)]
mod shared_corrections {
    use super::*;

    fn comparison(code: &mut Vec<u8>, selector: u32, target: u8) {
        code.push(0x63);
        code.extend(selector.to_be_bytes());
        code.extend([0x14, 0x60, target, 0x57]);
    }

    #[test]
    fn capped_dispatcher_does_not_claim_a_fallback() {
        let mut code = vec![0x5b, 0x00];
        for selector in 0..4097 {
            comparison(&mut code, selector, 0);
        }
        code.extend([0x5f, 0x5f, 0xfd]);
        // An intentional shared bug fix, not byte-for-byte legacy parity.
        assert_eq!(legacy::analyze(&code).unwrap().fallback, "present");
        let report = analyze(&code).unwrap();
        assert_eq!(report.functions.len(), 4096);
        assert_eq!(report.fallback, "unknown");
    }

    #[test]
    fn payability_requires_a_valid_jump_target() {
        for (tail, expected) in [(&[0x5b, 0x00][..], "payable"), (&[0x00][..], "unknown")] {
            let mut code = Vec::new();
            comparison(&mut code, 0x12345678, 10);
            code.extend([0x00, 0x5b, 0x60, 14, 0x56]);
            code.extend(tail);
            assert_eq!(legacy::analyze(&code).unwrap().functions[0].mutability, "payable");
            assert_eq!(analyze(&code).unwrap().functions[0].mutability, expected);
        }
    }
}
