//! Shared DEX reader with the app's error and checked-reader compatibility.
use crate::{FormatError, bytes::Reader};
pub use craft_dex::{
    ClassData, ClassDef, Code, Dex, EncodedField, EncodedMethod, Handler, Header, MemberId, NO_INDEX, Reference, Try, access_flags, is_dex, java_name,
    references, to_descriptor,
};
pub fn parse(data: &[u8]) -> Result<Dex<'_>, FormatError> {
    craft_dex::parse(data).map_err(|error| match error {
        craft_dex::DexError::Truncated(message) => FormatError::Truncated(message),
        craft_dex::DexError::Malformed(message) => FormatError::Malformed(message),
        craft_dex::DexError::Unsupported(message) => FormatError::Unsupported(message),
    })
}
pub fn mutf8(r: &Reader<'_>, at: usize, max_units: usize) -> (String, usize) {
    craft_dex::mutf8(&craft_dex::Reader::new(r.data()), at, max_units)
}
pub fn sleb128(r: &Reader<'_>, off: usize) -> Option<(i64, usize)> {
    craft_dex::sleb128(&craft_dex::Reader::new(r.data()), off)
}
#[cfg(test)]
#[path = "dex_legacy.rs"]
mod legacy;
#[cfg(test)]
mod parity {
    use super::*;
    #[test]
    fn shared_reader_and_opcodes_match() {
        let data = craft_anything_testkit::android::notes_dex();
        let mut samples = craft_anything_testkit::mutations(&data);
        samples.push(data);
        for data in samples {
            assert_eq!(legacy::is_dex(&data), is_dex(&data));
            match (legacy::parse(&data), parse(&data)) {
                (Ok(a), Ok(b)) => {
                    assert_eq!(serde_json::to_value(&a.header).unwrap(), serde_json::to_value(&b.header).unwrap());
                    assert_eq!(a.strings, b.strings);
                    for index in [0, 1, a.header.proto_ids.saturating_sub(1), a.header.proto_ids, u32::MAX] {
                        assert_eq!(a.shorty(index), b.shorty(index));
                    }
                    assert_eq!(a.warnings, b.warnings);
                    assert_eq!(format!("{:?}", a.classes), format!("{:?}", b.classes));
                    for (a_class, b_class) in a.classes.iter().zip(&b.classes) {
                        let old = a.class_data(a_class);
                        let new = b.class_data(b_class);
                        assert_eq!(format!("{old:?}"), format!("{new:?}"));
                        for (old_method, new_method) in
                            old.direct_methods.iter().chain(&old.virtual_methods).zip(new.direct_methods.iter().chain(&new.virtual_methods))
                        {
                            assert_eq!(a.method_signature(old_method), b.method_signature(new_method));
                            match (a.code(old_method.code_off), b.code(new_method.code_off)) {
                                (Some(old), Some(new)) => {
                                    assert_eq!(format!("{old:?}"), format!("{new:?}"));
                                    let old = crate::dex_ops::legacy::decode(&old.insns, 1000);
                                    let new = crate::dex_ops::decode(&new.insns, 1000);
                                    assert_eq!(serde_json::to_value(old).unwrap(), serde_json::to_value(new).unwrap());
                                }
                                (None, None) => {}
                                _ => panic!("DEX code mismatch"),
                            }
                        }
                    }
                }
                (Err(a), Err(b)) => assert_eq!(a, b),
                _ => panic!("DEX admission mismatch"),
            }
        }
    }
}
