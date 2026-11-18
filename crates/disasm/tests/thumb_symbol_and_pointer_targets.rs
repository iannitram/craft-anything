//! Regressions for ARM Thumb (`Mode::T32`) mode selection on entryless shared libraries and
//! low-bit (`addr | 1`) normalization across noreturn symbols, exports, stripped entry points,
//! and data-section function pointers.

use craft_anything_disasm::{Image, Limits, Mode, NoControl, Program, Xref, XrefKind};
use craft_anything_formats::{Arch, Binary, Export, Format, Section, Symbol, SymbolKind};
use std::sync::Arc;

/// Build a synthetic two-section (`.text` at `0x1000`, `.data` at `0x2000`) image for `arch`.
fn make_code_and_data_binary(arch: Arch, format: Format, entry: Option<u64>, code: &[u8], data: &[u8]) -> (Arc<[u8]>, Binary) {
    let mut raw = vec![0u8; 0x200 + data.len()];
    let code_len = code.len().min(0x100);
    raw[0x100..0x100 + code_len].copy_from_slice(&code[..code_len]);
    raw[0x200..0x200 + data.len()].copy_from_slice(data);

    let mut bin = Binary::raw(arch, 0, 0, 0);
    bin.format = format;
    bin.entry = entry;
    bin.sections = vec![
        Section {
            name: ".text".into(),
            segment: None,
            addr: 0x1000,
            size: code_len as u64,
            file_offset: Some(0x100),
            file_size: code_len as u64,
            read: true,
            write: false,
            exec: true,
            kind: "code",
            entropy: None,
            alloc: true,
        },
        Section {
            name: ".data".into(),
            segment: None,
            addr: 0x2000,
            size: data.len() as u64,
            file_offset: Some(0x200),
            file_size: data.len() as u64,
            read: true,
            write: true,
            exec: false,
            kind: "data",
            entropy: None,
            alloc: true,
        },
    ];
    (Arc::from(raw), bin)
}

/// Construct a defined function symbol at `addr`.
fn func_sym(name: &str, addr: u64, global: bool) -> Symbol {
    Symbol { name: name.into(), demangled: None, addr, size: 0, kind: SymbolKind::Function, global, weak: false, defined: true, dynamic: false }
}

/// Entryless ARM shared libraries (`e_entry == 0`, as in stripped Android `armeabi-v7a` `.so`
/// files) select `Mode::T32` from Thumb-bit executable exports/symbols and retain export names,
/// while odd-address data exports never flip an A32 library to Thumb.
#[test]
fn thumb_shared_library_without_entry_selects_t32_and_names_exports() {
    // 0x1000 <Java_pkg_add>: push {r7, lr}; mov r7, sp; add r0, r1; adds r0, #1; pop {r7, pc}
    // 0x100a <Java_pkg_seven>: movs r0, #7; bx lr
    let thumb_code: &[u8] = &[
        0x80, 0xb5, 0x6f, 0x46, 0x08, 0x44, 0x01, 0x30, 0x80, 0xbd, // 0x1000..0x100a
        0x07, 0x20, 0x70, 0x47, // 0x100a..0x100e
    ];
    let (bytes, mut bin) = make_code_and_data_binary(Arch::Arm, Format::Elf, None, thumb_code, &[0u8; 8]);
    bin.exports = vec![
        Export { name: "Java_pkg_add".into(), addr: 0x1001 },
        Export { name: "add_alias".into(), addr: 0x1001 },
        Export { name: "Java_pkg_seven".into(), addr: 0x100b },
        Export { name: "g_odd_byte".into(), addr: 0x2001 },
    ];

    let img = Image::from_binary(bytes, bin);
    assert_eq!(img.mode, Some(Mode::T32));
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze Thumb .so");

    let add = prog.functions.get(&0x1000).expect("discovered exported Thumb function at 0x1000");
    assert_eq!((add.name.as_str(), add.name_source), ("Java_pkg_add", "export"));
    assert!(add.returns && add.insn_count == 5, "{add:?}");
    assert_eq!(prog.function_by_name("Java_pkg_add").map(|f| f.entry), Some(0x1000));
    assert_eq!(prog.function_by_name("add_alias").map(|f| f.entry), Some(0x1000));

    let seven = prog.functions.get(&0x100a).expect("discovered exported Thumb function at 0x100a");
    assert_eq!((seven.name.as_str(), seven.name_source), ("Java_pkg_seven", "export"));

    // Control 1: an entryless A32 library with odd-address `.data` exports stays `Mode::A32`.
    let a32_code: Vec<u8> = [0xe3a0_0007u32, 0xe12f_ff1e].into_iter().flat_map(u32::to_le_bytes).collect();
    let (a32_bytes, mut a32_bin) = make_code_and_data_binary(Arch::Arm, Format::Elf, None, &a32_code, &[0u8; 8]);
    a32_bin.exports = vec![
        Export { name: "arm_leaf".into(), addr: 0x1000 },
        Export { name: "odd_data_1".into(), addr: 0x2001 },
        Export { name: "odd_data_2".into(), addr: 0x2003 },
    ];
    assert_eq!(Image::from_binary(a32_bytes, a32_bin).mode, Some(Mode::A32));

    // Control 2: duplicate listings across `symbols` and `exports` count once per function entry.
    let (mix_bytes, mut mix_bin) = make_code_and_data_binary(Arch::Arm, Format::Elf, None, &[0u8; 0x40], &[0u8; 4]);
    mix_bin.symbols = vec![
        func_sym("arm_wrap_1", 0x1000, true),
        func_sym("arm_wrap_2", 0x1008, true),
        func_sym("thumb_fn_1", 0x1011, false),
        func_sym("thumb_fn_2", 0x1019, false),
        func_sym("thumb_fn_3", 0x1021, false),
    ];
    mix_bin.exports = vec![Export { name: "arm_wrap_1".into(), addr: 0x1000 }, Export { name: "arm_wrap_2".into(), addr: 0x1008 }];
    assert_eq!(Image::from_binary(mix_bytes, mix_bin).mode, Some(Mode::T32));

    // Control 3: when an untagged symbol and a Thumb-tagged export/start refer to the same code
    // address, the Thumb tag wins regardless of listing order.
    let (up2_bytes, mut up2_bin) = make_code_and_data_binary(Arch::Arm, Format::Elf, None, &[0u8; 0x20], &[0u8; 4]);
    up2_bin.symbols = vec![func_sym("untagged_halfword", 0x1002, true)];
    up2_bin.exports = vec![Export { name: "untagged_halfword".into(), addr: 0x1003 }];
    assert_eq!(Image::from_binary(up2_bytes, up2_bin).mode, Some(Mode::T32));

    let (up4_bytes, mut up4_bin) = make_code_and_data_binary(Arch::Arm, Format::Elf, None, &[0u8; 0x20], &[0u8; 4]);
    up4_bin.symbols = vec![func_sym("untagged_word", 0x1000, true)];
    up4_bin.exports = vec![Export { name: "untagged_word".into(), addr: 0x1001 }];
    assert_eq!(Image::from_binary(up4_bytes, up4_bin).mode, Some(Mode::T32));
}

/// Thumb `noreturn` function symbols carry bit 0 = 1 in `.symtab`/`.dynsym`; `Program::analyze`
/// must match them against even `bl` call targets so unconditional calls stop scanning before
/// trailing bytes.
#[test]
fn thumb_noreturn_symbols_stop_unconditional_fallthrough() {
    // 0x1000 <fatal_caller>: push {r7, lr}; bl 0x1010;
    // 0x1006 (unreachable trailing code): movs r0, #99; pop {r7, pc}
    // 0x1010 <abort>: udf #0
    let mut code = vec![0u8; 0x20];
    code[0x00..0x06].copy_from_slice(&[0x80, 0xb5, 0x00, 0xf0, 0x05, 0xf8]); // push {r7, lr}; bl 0x1010
    code[0x06..0x0a].copy_from_slice(&[0x63, 0x20, 0x80, 0xbd]); // movs r0, #99; pop {r7, pc}
    code[0x10..0x12].copy_from_slice(&[0x00, 0xde]); // udf #0

    let (bytes, mut bin) = make_code_and_data_binary(Arch::Arm, Format::Elf, Some(0x1001), &code, &[]);
    bin.symbols = vec![func_sym("fatal_caller", 0x1001, true), func_sym("abort", 0x1011, true)];

    let img = Image::from_binary(bytes, bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze Thumb noreturn");
    let caller = prog.functions.get(&0x1000).expect("fatal_caller");
    assert_eq!(caller.insn_count, 2, "must stop at bl abort instead of falling through: {caller:?}");
    assert!(!caller.contains(0x1006), "{:?}", caller.blocks);
    assert!(!caller.returns, "{caller:?}");
}

/// Stripped Thumb entry points keep the `"entry"` name (`"entry_point"`), and 32-bit function
/// pointers stored with the Thumb bit (`target | 1`) in data sections produce `Pointer` xrefs to
/// the function's even entry without disturbing odd-address string pointers.
#[test]
fn thumb_stripped_entry_and_data_function_pointers_normalize_low_bit() {
    // 0x1000: bl 0x1010; bx lr
    // 0x1010: movs r0, #5; bx lr
    let mut code = vec![0u8; 0x20];
    code[0x00..0x06].copy_from_slice(&[0x00, 0xf0, 0x06, 0xf8, 0x70, 0x47]);
    code[0x10..0x14].copy_from_slice(&[0x05, 0x20, 0x70, 0x47]);

    // .data at 0x2000:
    //   0x2000: 0x00001011 (Thumb function pointer to 0x1010)
    //   0x2004: 0x00002009 (pointer to string at odd address 0x2009)
    //   0x2009..: "hello_thumb\0"
    let mut data = vec![0u8; 0x20];
    data[0x00..0x04].copy_from_slice(&0x0000_1011u32.to_le_bytes());
    data[0x04..0x08].copy_from_slice(&0x0000_2009u32.to_le_bytes());
    data[0x09..0x15].copy_from_slice(b"hello_thumb\0");

    let (bytes, bin) = make_code_and_data_binary(Arch::Arm, Format::Elf, Some(0x1001), &code, &data);
    let img = Image::from_binary(bytes, bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze stripped Thumb ELF");

    let entry = prog.functions.get(&0x1000).expect("entry function at 0x1000");
    assert_eq!((entry.name.as_str(), entry.name_source), ("entry", "entry_point"));
    assert_eq!(prog.function_by_name("entry").map(|f| f.entry), Some(0x1000));

    let ptr_xrefs = prog.xrefs_to.get(&0x1010).expect("xrefs to 0x1010");
    assert!(ptr_xrefs.contains(&Xref { from: 0x2000, to: 0x1010, kind: XrefKind::Pointer }), "expected Pointer xref from 0x2000 to 0x1010, got: {ptr_xrefs:?}");

    let str_xrefs = prog.xrefs_to.get(&0x2009).expect("xrefs to odd string 0x2009");
    assert!(str_xrefs.contains(&Xref { from: 0x2004, to: 0x2009, kind: XrefKind::String }), "expected String xref from 0x2004 to 0x2009, got: {str_xrefs:?}");
}

/// On byte-addressed ISAs (`Mode::X64`), a 1-byte function at an even address (`0x1000: ret`)
/// must not steal the symbol name of an adjacent function starting at `0x1001`.
#[test]
fn x86_adjacent_one_byte_functions_do_not_steal_odd_symbol_names() {
    // 0x1000: ret (1 byte)
    // 0x1001 <odd_neighbor>: mov eax, 1; ret
    let code: &[u8] = &[0xc3, 0xb8, 0x01, 0x00, 0x00, 0x00, 0xc3];
    let (bytes, mut bin) = make_code_and_data_binary(Arch::X86_64, Format::Elf, Some(0x1000), code, &[]);
    bin.bits = 64;
    bin.symbols = vec![func_sym("odd_neighbor", 0x1001, true)];

    let img = Image::from_binary(bytes, bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze x86_64");

    let first = prog.functions.get(&0x1000).expect("0x1000");
    let second = prog.functions.get(&0x1001).expect("0x1001");
    assert_eq!((first.name.as_str(), first.name_source), ("entry", "entry_point"));
    assert_eq!((second.name.as_str(), second.name_source), ("odd_neighbor", "symbol"));
}
