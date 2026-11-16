//! Pure, standalone file-format parsers. No workspace dependencies, no panics on any input.
//! Every parser takes bytes and returns a `Result`; sizes and offsets from the input are checked.
//! The one exception to bytes in memory is [`zip_file`], which reads an entry from a seekable file.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod ai_model;
pub mod apk_sign;
pub mod arsc;
pub mod asar;
pub mod authenticode;
pub mod axml;
pub mod binary;
mod bptc;
pub mod bun_program;
pub mod bytes;
pub mod car;
pub mod codesign;
mod common;
pub mod dart;
pub mod decompress;
pub mod der;
pub mod dex;
pub mod dex_ops;
pub mod dos;
pub mod dotnet;
pub mod dotnet_bundle;
pub mod elf;
pub mod evm;
pub mod firmware;
pub mod fsread;
pub mod go;
pub mod godot_pck;
pub mod har;
pub mod hints;
pub mod il2cpp;
pub mod java_class;
pub mod java_ops;
pub mod jsonc;
pub mod keyed_archive;
pub mod macho;
pub mod macho_deps;
pub mod nib;
pub mod node_sea;
pub mod packed;
mod packed_carrier;
pub mod pcap;
pub mod pe;
pub mod pixels;
pub mod pkcs7;
pub mod plist;
pub mod pyc;
pub mod pyinstaller;
pub mod rust_bin;
pub mod solana;
pub mod sourcemap;
pub mod sqlite;
pub mod strings;
pub mod swift;
pub mod swift_demangle;
pub mod tarcpio;
pub mod unity_audio;
pub mod unity_bundle;
pub mod unity_serialized;
pub mod unity_texture;
pub mod unreal_iostore;
pub mod unreal_pak;
pub mod unreal_texture;
pub mod verify;
pub mod wasm;
pub mod zip;
pub mod zip_file;

// Native executable formats (ELF, PE, Mach-O, DOS) and their shared helpers.
pub use binary::{Arch, Binary, Export, Format, Import, ImportSlot, Mitigations, Relocation, Section, Segment, Symbol, SymbolKind, parse_binary};
pub use common::{FormatError, entropy, find_bytes, hex_addr};

use bytes::Reader;

/// True when a PE file carries a CLI (.NET) header: data directory 14 is non-empty.
pub fn pe_is_managed(data: &[u8]) -> bool {
    let r = Reader::new(data);
    let Some(pe_off) = r.u32_le(0x3c).map(|v| v as usize) else { return false };
    if r.slice(pe_off, 4) != Some(b"PE\0\0") {
        return false;
    }
    let Some(coff) = r.slice(pe_off, 24).map(Reader::new) else { return false };
    let Some(size) = coff.u16_le(20).map(usize::from) else { return false };
    let Some(opt) = pe_off.checked_add(24) else { return false };
    let Some(r) = r.slice(opt, size).map(Reader::new) else { return false };
    let dirs = match r.u16_le(0) {
        Some(0x10b) => 96usize,
        Some(0x20b) => 112usize,
        _ => return false,
    };
    let Some(count) = r.u32_le(dirs.wrapping_sub(4)) else { return false };
    if count <= 14 {
        return false;
    }
    let entry = dirs + 14 * 8;
    matches!((r.u32_le(entry), r.u32_le(entry + 4)), (Some(rva), Some(size)) if rva != 0 && size != 0)
}

/// True when the ZIP's own central directory names `entry`, without decompressing members.
pub fn zip_has_entry(data: &[u8], entry: &str) -> bool {
    zip::has_entry(data, entry)
}

/// The ASAR reader's own check, so format dispatch and the reader agree on what an ASAR is.
pub use asar::looks_like_asar;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_inputs_do_not_panic() {
        for len in 0..200 {
            let junk: Vec<u8> = (0..len).map(|i| (i * 37 % 251) as u8).collect();
            let _ = pe_is_managed(&junk);
            let _ = zip_has_entry(&junk, "AndroidManifest.xml");
            let _ = looks_like_asar(&junk);
        }
        let mut mz = b"MZ".to_vec();
        mz.resize(0x40, 0);
        mz[0x3c] = 0xff;
        assert!(!pe_is_managed(&mz));
    }

    #[test]
    fn asar_header() {
        assert!(looks_like_asar(&asar::build(&[("a.js", b"a();")])));
    }
}
