//! Shared class-file reader with unchanged application errors.
use crate::FormatError;
pub use craft_java_class::{
    AccessKind, ClassFile, Code, Const, ExceptionEntry, InnerClass, Member, Ref, Reference, Version, access_flags, field_type, internal_to_dotted, is_class,
    method_params, references,
};
pub fn parse(data: &[u8]) -> Result<ClassFile, FormatError> {
    craft_java_class::parse(data).map_err(|error| match error {
        craft_java_class::JavaClassError::Truncated(message) => FormatError::Truncated(message),
        craft_java_class::JavaClassError::Malformed(message) => FormatError::Malformed(message),
        craft_java_class::JavaClassError::Unsupported(message) => FormatError::Unsupported(message),
    })
}
#[cfg(test)]
#[path = "java_class_legacy.rs"]
mod legacy;
#[cfg(test)]
mod parity {
    use super::*;
    #[test]
    fn shared_class_reports_errors_and_opcodes_match() {
        let data = craft_anything_testkit::java::hello_class();
        let mut samples = craft_anything_testkit::mutations(&data);
        samples.push(data);
        for data in samples {
            assert_eq!(is_class(&data), legacy::is_class(&data));
            match (legacy::parse(&data), parse(&data)) {
                (Ok(a), Ok(b)) => {
                    assert_eq!(format!("{a:?}"), format!("{b:?}"));
                    assert_eq!(a.access, b.access);
                    assert_eq!(a.signature, b.signature);
                    assert_eq!(a.annotations, b.annotations);
                    assert_eq!(serde_json::to_value(&a.inner_classes).unwrap(), serde_json::to_value(&b.inner_classes).unwrap());
                    for index in 0..a.pool.len() {
                        assert_eq!(a.name_and_type_of_indy(index as u16), b.name_and_type_of_indy(index as u16));
                    }
                    assert_eq!(legacy::access_flags(a.access, legacy::AccessKind::Field), access_flags(b.access, AccessKind::Field));
                    assert_eq!(legacy::access_flags(a.access, legacy::AccessKind::InnerClass), access_flags(b.access, AccessKind::InnerClass));

                    for owner in ["java/lang/Object", "com/example/Hello", "java/lang/String"] {
                        for cap in [0, 1, 1024] {
                            assert_eq!(
                                serde_json::to_value(legacy::references(&a, owner, None, cap)).unwrap(),
                                serde_json::to_value(references(&b, owner, None, cap)).unwrap()
                            );
                        }
                    }
                    for (a, b) in a.methods.iter().zip(&b.methods) {
                        if let (Some(a), Some(b)) = (&a.code, &b.code) {
                            assert_eq!(
                                serde_json::to_value(crate::java_ops::legacy::decode(&a.bytecode, 1024)).unwrap(),
                                serde_json::to_value(crate::java_ops::decode(&b.bytecode, 1024)).unwrap()
                            );
                        }
                    }
                }
                (Err(a), Err(b)) => assert_eq!(a, b),
                _ => panic!("Java class admission mismatch"),
            }
        }
    }
}
