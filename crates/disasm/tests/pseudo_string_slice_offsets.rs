//! Regression tests for byte-accurate UTF-8 and UTF-16LE string literal slicing in `pseudo::lift`.

use craft_anything_disasm::pseudo::lift;
use craft_anything_disasm::{Image, Limits, NoControl, Program};
use craft_anything_testkit as tk;
use std::sync::Arc;

/// Build an ELF64 x86-64 sample whose `.rodata` holds `rodata` and whose `main` passes `rodata_va + disp_off` to `puts`.
fn elf_referencing_rodata(rodata: &[u8], disp_off: u64) -> (Image, Program) {
    let (mut elf, layout) = tk::elf64(tk::ElfArch::X86_64, true);
    let r_off = tk::ELF_RODATA_OFF as usize;
    elf[r_off..r_off + tk::rodata::SIZE].fill(0);
    elf[r_off..r_off + rodata.len()].copy_from_slice(rodata);

    // main:
    //   0x00: push rbp; mov rbp, rsp
    //   0x04: lea rdi, [rip + target_str]
    //   0x0b: call [rip + got_puts]
    //   0x11: xor eax, eax; pop rbp; ret
    let target_va = layout.rodata_va + disp_off;
    let lea_disp = ((target_va as i64) - ((layout.code_va + 0x0b) as i64)) as i32;
    let call_disp = ((layout.got_va as i64) - ((layout.code_va + 0x11) as i64)) as i32;
    let mut code = Vec::new();
    code.extend_from_slice(&[0x55, 0x48, 0x89, 0xe5]);
    code.extend_from_slice(&[0x48, 0x8d, 0x3d]);
    code.extend_from_slice(&lea_disp.to_le_bytes());
    code.extend_from_slice(&[0xff, 0x15]);
    code.extend_from_slice(&call_disp.to_le_bytes());
    code.extend_from_slice(&[0x31, 0xc0, 0x5d, 0xc3]);

    let t_off = tk::ELF_TEXT_OFF as usize;
    elf[t_off..t_off + tk::x64::CODE_SIZE].fill(0xcc);
    elf[t_off..t_off + code.len()].copy_from_slice(&code);

    let img = Image::parse(Arc::from(elf)).unwrap();
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).unwrap();
    (img, prog)
}

/// Encode an ASCII/BMP string as NUL-terminated UTF-16LE bytes.
fn utf16le_nul(s: &str) -> Vec<u8> {
    let mut out: Vec<u8> = s.encode_utf16().flat_map(u16::to_le_bytes).collect();
    out.extend_from_slice(&[0, 0]);
    out
}

/// Referencing byte offset 0 of a UTF-16LE string inlines the full wide literal.
#[test]
fn utf16le_zero_offset_inlines_full_string() {
    let wide = utf16le_nul("prefix:hello");
    let (img, prog) = elf_referencing_rodata(&wide, 0);
    let main = prog.function_by_name("main").unwrap();
    let lifted = lift(&img, &prog, main, &|_| None);
    assert!(lifted.text.contains("puts(\"prefix:hello\")"), "{}", lifted.text);
}

/// Referencing an even interior byte offset of a UTF-16LE string slices by 2-byte UTF-16 code units, not chars.
#[test]
fn utf16le_interior_even_byte_offset_slices_code_units() {
    let wide = utf16le_nul("prefix:hello");
    // "prefix:" is 7 UTF-16LE code units = 14 bytes; tail at +14 bytes is L"hello".
    let (img, prog) = elf_referencing_rodata(&wide, 14);
    let main = prog.function_by_name("main").unwrap();
    let lifted = lift(&img, &prog, main, &|_| None);
    assert!(lifted.text.contains("puts(\"hello\")"), "expected +14 bytes into UTF-16LE `prefix:hello` to lift as \"hello\", got:\n{}", lifted.text);
    assert_eq!(lifted.strings, vec!["hello".to_string()]);
}

/// Referencing an odd misaligned byte offset inside a UTF-16LE string falls back to a data address.
#[test]
fn utf16le_odd_byte_offset_is_not_inlined_as_shifted_text() {
    let wide = utf16le_nul("prefix:hello");
    let (img, prog) = elf_referencing_rodata(&wide, 1);
    let main = prog.function_by_name("main").unwrap();
    let lifted = lift(&img, &prog, main, &|_| None);
    assert!(
        !lifted.text.contains("refix:hello") && lifted.strings.is_empty(),
        "misaligned +1 byte into UTF-16LE string must not inline a bogus literal:\n{}",
        lifted.text
    );
    assert!(lifted.text.contains("&data_"), "{}", lifted.text);
}

/// Multi-byte UTF-8 interior offsets slice by UTF-8 byte boundary and reject mid-codepoint addresses.
#[test]
fn utf8_multibyte_offsets_respect_byte_and_codepoint_boundaries() {
    let mut utf8 = "→hello".as_bytes().to_vec(); // '→' is 3 UTF-8 bytes (0xe2 0x86 0x92).
    utf8.push(0);

    // Mid-codepoint (+1 byte inside '→'): must not fabricate `"hello"`.
    let (img_mid, prog_mid) = elf_referencing_rodata(&utf8, 1);
    let lifted_mid = lift(&img_mid, &prog_mid, prog_mid.function_by_name("main").unwrap(), &|_| None);
    assert!(
        lifted_mid.strings.is_empty() && lifted_mid.text.contains("&data_"),
        "mid-codepoint +1 byte inside 3-byte UTF-8 char must not inline a string:\n{}",
        lifted_mid.text
    );

    // Exact codepoint boundary (+3 bytes, right after '→') when xrefs did not pre-split the string:
    let (mut elf, _) = tk::elf64(tk::ElfArch::X86_64, true);
    let r_off = tk::ELF_RODATA_OFF as usize;
    elf[r_off..r_off + tk::rodata::SIZE].fill(0);
    elf[r_off..r_off + utf8.len()].copy_from_slice(&utf8);
    let (img_exact, _) = elf_referencing_rodata(&utf8, 3);
    let capped = Limits { max_xrefs: 0, ..Limits::default() };
    let prog_unsplit = Program::analyze(&img_exact, &capped, &NoControl).unwrap();
    let lifted_exact = lift(&img_exact, &prog_unsplit, prog_unsplit.function_by_name("main").unwrap(), &|_| None);
    assert!(lifted_exact.text.contains("puts(\"hello\")"), "expected +3 bytes past 3-byte '→' to inline \"hello\" (not \"llo\"), got:\n{}", lifted_exact.text);
}
