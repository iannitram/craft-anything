//! Shared Dalvik opcode decoding.
pub use craft_dex::dex_ops::*;
#[cfg(test)]
#[path = "dex_ops_legacy.rs"]
pub(crate) mod legacy;
