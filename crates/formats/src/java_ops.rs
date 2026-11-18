//! Shared JVM opcode decoding.
pub use craft_java_class::java_ops::*;
#[cfg(test)]
#[path = "java_ops_legacy.rs"]
pub(crate) mod legacy;
