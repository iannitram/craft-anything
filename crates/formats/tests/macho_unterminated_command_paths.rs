use craft_anything_formats::{
    binary::{Arch, Binary},
    macho, macho_deps,
};
use craft_anything_testkit::{Buf, apple};

/// Remove the terminator and its padding from a load command with a path at the given offset.
fn unterminated(mut command: Vec<u8>, offset: usize) -> Vec<u8> {
    let end = offset + command[offset..].iter().position(|&byte| byte == 0).unwrap();
    command[end..].fill(b'x');
    command
}

/// Both views ignore malformed path bytes and still read the following valid commands.
#[test]
fn malformed_command_paths_do_not_become_dependencies() {
    let data = apple::macho_min(
        apple::CPU_X86_64,
        2,
        &[
            unterminated(apple::lc_dylib(0xc, "/bad.dylib"), 24),
            unterminated(apple::lc_rpath("/bad"), 12),
            apple::lc_dylib(0xc, "/good.dylib"),
            apple::lc_rpath("/good"),
        ],
        &[],
    );
    let deps = macho_deps::parse_thin(&data).unwrap();
    assert_eq!(deps.dylibs.iter().map(|dep| dep.name.as_str()).collect::<Vec<_>>(), ["/good.dylib"]);
    assert_eq!(deps.rpaths, ["/good"]);
    let mut binary = Binary::raw(Arch::X86_64, 0, 0, 0);
    macho::enrich(&data, &mut binary);
    assert_eq!(binary.libraries, ["/good.dylib"]);
    assert_eq!(binary.info.get("rpath").map(String::as_str), Some("/good"));
}

/// Both indirect symbols keep their original dylib ordinals even if ordinal one has no path.
fn assert_import_ordinals(first_dylib: Vec<u8>) {
    const SYMOFF: u32 = 0x210;
    const STROFF: u32 = 0x230;
    const INDOFF: u32 = 0x240;
    let strings = b"\0_first\0_second\0";

    let mut symtab = Buf::with_len(24);
    symtab.u32(0, 0x2); // LC_SYMTAB
    symtab.u32(4, 24);
    symtab.u32(8, SYMOFF);
    symtab.u32(12, 2);
    symtab.u32(16, STROFF);
    symtab.u32(20, strings.len() as u32);
    let mut dysymtab = Buf::with_len(80);
    dysymtab.u32(0, 0xb); // LC_DYSYMTAB
    dysymtab.u32(4, 80);
    dysymtab.u32(28, 2); // two undefined symbols
    dysymtab.u32(56, INDOFF);
    dysymtab.u32(60, 2);

    let mut data = Buf(apple::macho_min(
        apple::CPU_X86_64,
        2,
        &[first_dylib, apple::lc_dylib(0xc, "/good.dylib"), symtab.into_vec(), dysymtab.into_vec()],
        &[("__got", &[0; 16])],
    ));
    data.u32(32 + 72 + 64, 6); // S_NON_LAZY_SYMBOL_POINTERS in the first section
    for (index, strx, ordinal) in [(0, 1, 1), (1, 8, 2)] {
        let symbol = SYMOFF as usize + index * 16;
        data.u32(symbol, strx);
        data.u8(symbol + 4, 1); // undefined external symbol
        data.u16(symbol + 6, ordinal << 8);
        data.u32(INDOFF as usize + index * 4, index as u32);
    }
    data.put(STROFF as usize, strings);

    let mut binary = Binary::raw(Arch::X86_64, 0, 0, 0);
    macho::enrich(&data.into_vec(), &mut binary);
    assert_eq!(binary.libraries, ["/good.dylib"]);
    let imports: Vec<_> = binary.import_slots.iter().map(|slot| (slot.name.as_str(), slot.library.as_deref())).collect();
    assert_eq!(imports, [("_first", None), ("_second", Some("/good.dylib"))]);
}

/// Rejected unterminated names still occupy their slot in the import ordinal table.
#[test]
fn unterminated_dylib_name_preserves_import_ordinals() {
    assert_import_ordinals(unterminated(apple::lc_dylib(0xc, "/bad.dylib"), 24));
}

/// A recognized dylib command with missing fields also keeps its ordinal.
#[test]
fn undersized_dylib_command_preserves_import_ordinals() {
    let mut short = Buf::with_len(16);
    short.u32(0, 0xc);
    short.u32(4, 16);
    assert_import_ordinals(short.into_vec());
}
