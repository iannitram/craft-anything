//! Regressions verifying that pseudo-C call/tail-call and Win64 stack-argument folding does
//! not inline stale register expressions after one of their source registers is overwritten.

use craft_anything_disasm::pseudo::lift;
use craft_anything_disasm::{Image, Limits, NoControl, Program};
use craft_anything_formats::{Arch, Binary};
use craft_anything_testkit as tk;
use std::sync::Arc;

/// Copying an argument register (`mov rsi, rdi`) before updating the source (`add edi, 1`)
/// must keep `rsi = rdi;` intact instead of folding both call arguments to the updated `rdi`.
#[test]
fn x64_call_keeps_saved_arg_when_source_register_is_modified_before_call() {
    // 0x1000: mov rsi, rdi; add edi, 1; call 0x1020; ret
    // 0x1020: ret
    let mut code = vec![0xcc; 0x30];
    code[..12].copy_from_slice(&[
        0x48, 0x89, 0xfe, // mov rsi, rdi
        0x83, 0xc7, 0x01, // add edi, 1
        0xe8, 0x15, 0x00, 0x00, 0x00, // call 0x1020
        0xc3, // ret
    ]);
    code[0x20] = 0xc3;

    let mut bin = Binary::raw(Arch::X86_64, 0x1000, code.len() as u64, 0);
    bin.entry = Some(0x1000);
    let img = Image::from_binary(Arc::from(code), bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).unwrap();
    let lifted = lift(&img, &prog, prog.functions.get(&0x1000).unwrap(), &|_| None);

    assert!(lifted.text.contains("rsi = rdi;"), "expected saved rsi assignment to be preserved:\n{}", lifted.text);
    assert!(lifted.text.contains("sub_1020(rdi, rsi);"), "expected call to keep distinct rdi and rsi:\n{}", lifted.text);
}

/// Copying `rdi` into `rsi`, reloading `edi = 42`, and passing `rdx = rdi` must not fold `rsi`
/// as `rdi` after `edi = 42` overwrote `rdi`.
#[test]
fn x64_call_does_not_fold_earlier_register_copy_across_overwrite() {
    // 0x1000: mov rsi, rdi; mov edi, 42; mov rdx, rdi; call 0x1020; ret
    // 0x1020: ret
    let mut code = vec![0xcc; 0x30];
    code[..17].copy_from_slice(&[
        0x48, 0x89, 0xfe, // mov rsi, rdi
        0xbf, 0x2a, 0x00, 0x00, 0x00, // mov edi, 42
        0x48, 0x89, 0xfa, // mov rdx, rdi
        0xe8, 0x0b, 0x00, 0x00, 0x00, // call 0x1020 (next IP is 0x1010, +0x10 -> 0x1020)
        0xc3, // ret
    ]);
    code[0x20] = 0xc3;

    let mut bin = Binary::raw(Arch::X86_64, 0x1000, code.len() as u64, 0);
    bin.entry = Some(0x1000);
    let img = Image::from_binary(Arc::from(code), bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).unwrap();
    let lifted = lift(&img, &prog, prog.functions.get(&0x1000).unwrap(), &|_| None);

    assert!(lifted.text.contains("rsi = rdi;"), "expected saved rsi assignment before overwrite:\n{}", lifted.text);
    assert!(!lifted.text.contains("(rdi, rdi, rdi)"), "stale rdi folded into rsi:\n{}", lifted.text);
}

/// On AArch64, saving `x1 = x0` before modifying `w0` for either a `bl` call or a tail `b`
/// keeps `x1 = x0;` alive so the callee receives distinct updated and original values.
#[test]
fn aarch64_call_and_tail_call_preserve_saved_register_when_source_updates() {
    // 0x1000: ret (helper)
    // 0x1020 (entry): mov x1, x0; add w0, w0, #1; bl 0x1000; mov x1, x0; add w0, w0, #1; b 0x1000
    let mut words = [0xd503_201fu32; 16];
    words[0] = 0xd65f_03c0; // 0x1000: ret
    words[8] = 0xaa00_03e1; // 0x1020: mov x1, x0
    words[9] = 0x1100_0400; // 0x1024: add w0, w0, #1
    words[10] = 0x97ff_fff6; // 0x1028: bl 0x1000 (-10 insns)
    words[11] = 0xaa00_03e1; // 0x102c: mov x1, x0
    words[12] = 0x1100_0400; // 0x1030: add w0, w0, #1
    words[13] = 0x17ff_fff3; // 0x1034: b 0x1000 (-13 insns -> tail call)

    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut bin = Binary::raw(Arch::Aarch64, 0x1000, bytes.len() as u64, 0);
    bin.entry = Some(0x1020);
    let img = Image::from_binary(Arc::from(bytes), bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).unwrap();
    let lifted = lift(&img, &prog, prog.functions.get(&0x1020).unwrap(), &|_| None);

    assert_eq!(lifted.text.matches("x1 = x0;").count(), 2, "expected both saved x1 assignments:\n{}", lifted.text);
    assert!(lifted.text.contains("sub_1000(x0, x1);"), "expected bl call to pass distinct x0 and x1:\n{}", lifted.text);
    assert!(lifted.text.contains("return sub_1000(x0, x1); /* tail call */"), "expected tail call to pass distinct x0 and x1:\n{}", lifted.text);
}

/// Win64 stack arguments stored from a register (`mov [rsp+0x20], rcx`) must not be folded into
/// the import call if `rcx` is modified before the call.
#[test]
fn win64_stack_arg_is_not_folded_after_its_source_register_changes() {
    let mut pe = tk::pe64();
    // main at 0x140001000; puts IAT slot at PE_BASE + PE_IAT_RVA (0x140002160).
    // `call` starts at offset 29 (len 6 -> next RIP 0x140001023), disp = 0x2160 - 0x1023 = 0x113d.
    let code: [u8; 40] = [
        0x48, 0x83, 0xec, 0x48, // sub rsp, 0x48
        0x48, 0x89, 0x4c, 0x24, 0x20, // mov [rsp+0x20], rcx
        0x83, 0xc1, 0x01, // add ecx, 1
        0xba, 0x02, 0x00, 0x00, 0x00, // mov edx, 2
        0x41, 0xb8, 0x03, 0x00, 0x00, 0x00, // mov r8d, 3
        0x41, 0xb9, 0x04, 0x00, 0x00, 0x00, // mov r9d, 4
        0xff, 0x15, 0x3d, 0x11, 0x00, 0x00, // call [rip+0x113d] (puts IAT slot)
        0x48, 0x83, 0xc4, 0x48, // add rsp, 0x48
        0xc3, // ret
    ];
    pe[0x400..0x400 + code.len()].copy_from_slice(&code);
    let img = Image::parse(Arc::from(pe)).unwrap();
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).unwrap();
    let main = prog.functions.get(&(tk::PE_BASE + tk::PE_TEXT_RVA)).unwrap();
    let out = lift(&img, &prog, main, &|_| None);

    assert!(out.text.contains("stack_20 = rcx;"), "expected stack arg assignment to stay visible:\n{}", out.text);
    assert!(out.text.contains("puts(rcx, 2, 3, 4, stack_20);"), "expected puts to receive stack_20 rather than modified rcx:\n{}", out.text);
}
