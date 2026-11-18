//! Regression tests for versioned, stubbed, and Darwin-variant import arity resolution.

use craft_anything_disasm::arity::import_arity;
use craft_anything_disasm::pseudo::lift;
use craft_anything_disasm::{Image, Limits, NoControl, Program};
use craft_anything_testkit as tk;
use std::sync::Arc;

/// Unversioned C, Mach-O, and Win32 stdcall spellings retain their expected argument counts.
#[test]
fn plain_and_stdcall_imports_keep_expected_arity() {
    assert_eq!(import_arity("memcpy"), Some(3));
    assert_eq!(import_arity("_fopen"), Some(2));
    assert_eq!(import_arity("__imp_MessageBoxW"), Some(4));
    assert_eq!(import_arity("_CustomStdcall@12"), Some(3));
    assert_eq!(import_arity("odd@name"), None);
}

/// GNU symbol-versioned, PLT/stub-suffixed, and Darwin `$VARIANT` imports resolve to table arity.
#[test]
fn versioned_stubbed_and_darwin_variant_names_resolve_arity() {
    assert_eq!(import_arity("memcpy@GLIBC_2.14"), Some(3));
    assert_eq!(import_arity("puts@@GLIBC_2.2.5"), Some(1));
    assert_eq!(import_arity("pthread_create@GLIBC_2.34"), Some(4));
    assert_eq!(import_arity("fopen@plt"), Some(2));
    assert_eq!(import_arity("_fopen@stub"), Some(2));
    assert_eq!(import_arity("CreateFileW@thunk"), Some(7));
    assert_eq!(import_arity("_fopen$INODE64"), Some(2));
    assert_eq!(import_arity("_write$UNIX2003"), Some(3));
    assert_eq!(import_arity("_pthread_create$NOCANCEL"), Some(4));
    assert_eq!(import_arity("unknown_fn@GLIBC_2.2.5"), None);
    assert_eq!(import_arity("_unknown_fn$UNIX2003"), None);
}

/// Pseudo-C lifting preserves cross-block register arguments when calling a versioned ELF import.
#[test]
fn pseudo_c_retains_cross_block_arguments_for_versioned_elf_import() {
    let (mut elf, layout) = tk::elf64(tk::ElfArch::X86_64, true);
    // Assemble `main` with two predecessors so `rsi` and `rdx` are set before a branch while only
    // `edi` is assigned inside the call block:
    //   0x00: push rbp; mov rbp, rsp
    //   0x04: mov edx, 16
    //   0x09: test edi, edi
    //   0x0b: je +2 (0x0f)
    //   0x0d: mov esi, 9
    //   0x12: mov edi, 1
    //   0x17: call [rip + disp_to_got]
    //   0x1d: pop rbp; ret
    let call_next = layout.code_va + 0x1d;
    let disp = ((layout.got_va as i64) - (call_next as i64)) as i32;
    let mut code = Vec::new();
    code.extend_from_slice(&[0x55, 0x48, 0x89, 0xe5]);
    code.extend_from_slice(&[0xba, 0x10, 0x00, 0x00, 0x00]); // mov edx, 16
    code.extend_from_slice(&[0x85, 0xff]); // test edi, edi
    code.extend_from_slice(&[0x74, 0x05]); // je +5 -> 0x12
    code.extend_from_slice(&[0xbe, 0x09, 0x00, 0x00, 0x00]); // mov esi, 9
    code.extend_from_slice(&[0xbf, 0x01, 0x00, 0x00, 0x00]); // mov edi, 1
    code.extend_from_slice(&[0xff, 0x15]);
    code.extend_from_slice(&disp.to_le_bytes());
    code.extend_from_slice(&[0x5d, 0xc3]);

    let off = tk::ELF_TEXT_OFF as usize;
    elf[off..off + code.len()].copy_from_slice(&code);

    let mut img = Image::parse(Arc::from(elf)).unwrap();
    img.bin.import_slots[0].name = "memcpy@GLIBC_2.14".into();

    let prog = Program::analyze(&img, &Limits::default(), &NoControl).unwrap();
    let main = prog.function_by_name("main").unwrap();
    let lifted = lift(&img, &prog, main, &|_| None);
    assert!(
        lifted.text.contains("memcpy@GLIBC_2.14(1, rsi, rdx)"),
        "expected all 3 arguments of versioned memcpy import to be preserved across blocks, got:\n{}",
        lifted.text
    );
}
