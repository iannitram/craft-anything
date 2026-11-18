//! Regressions verifying that `Flow::IndirectCall` preserves assignments to registers read by the
//! call target / memory address / pointer-authentication modifier instead of folding them as fake
//! callee arguments and deleting the assignment statement in pseudo-C.

use craft_anything_disasm::pseudo::lift;
use craft_anything_disasm::{Image, Limits, NoControl, Program};
use craft_anything_formats::{Arch, Binary, ImportSlot, Symbol, SymbolKind};
use craft_anything_testkit as tk;
use std::sync::Arc;

/// ARM32 (`blx r3`) and Thumb (`blx r2`) indirect calls using an AAPCS32 argument register for the
/// function pointer must keep `r3 = ...;` / `r2 = ...;` alive and pass only the real arguments.
#[test]
fn arm_and_thumb_indirect_call_keeps_target_register_out_of_arguments() {
    // A32 at 0x1000:
    //   0x1000: push {r4, lr}       (e92d4010)
    //   0x1004: mov r0, #1          (e3a00001)
    //   0x1008: mov r1, #2          (e3a01002)
    //   0x100c: ldr r3, [pc, #4]    (e59f3004 -> 0x1018)
    //   0x1010: blx r3              (e12fff33)
    //   0x1014: pop {r4, pc}        (e8bd8010)
    //   0x1018: .word 0xdeadbeef
    let a32: Vec<u8> =
        [0xe92d_4010u32, 0xe3a0_0001, 0xe3a0_1002, 0xe59f_3004, 0xe12f_ff33, 0xe8bd_8010, 0xdead_beef].into_iter().flat_map(u32::to_le_bytes).collect();
    let mut bin_a32 = Binary::raw(Arch::Arm, 0x1000, a32.len() as u64, 0);
    bin_a32.entry = Some(0x1000);
    let img_a32 = Image::from_binary(Arc::from(a32), bin_a32);
    let prog_a32 = Program::analyze(&img_a32, &Limits::default(), &NoControl).expect("analyze A32");
    let out_a32 = lift(&img_a32, &prog_a32, prog_a32.functions.get(&0x1000).expect("A32 entry"), &|_| None);

    assert!(out_a32.text.contains("r3 = *(uint32_t*)0x1018;"), "A32 target load must remain visible:\n{}", out_a32.text);
    assert!(out_a32.text.contains("(*r3)(1, 2);"), "A32 blx r3 must pass only r0, r1:\n{}", out_a32.text);

    // Thumb at 0x1000 (entry = 0x1001):
    //   0x1000: push {r4, lr}       (10 b5)
    //   0x1002: movs r0, #7         (07 20)
    //   0x1004: ldr r2, [pc, #4]    (01 4a -> Align(0x1008, 4) + 4 = 0x100c)
    //   0x1006: blx r2              (90 47)
    //   0x1008: pop {r4, pc}        (10 bd)
    //   0x100a: nop                 (00 bf)
    //   0x100c: .word 0x00002000
    let thumb = vec![0x10, 0xb5, 0x07, 0x20, 0x01, 0x4a, 0x90, 0x47, 0x10, 0xbd, 0x00, 0xbf, 0x00, 0x20, 0x00, 0x00];
    let mut bin_t32 = Binary::raw(Arch::Arm, 0x1000, thumb.len() as u64, 0);
    bin_t32.entry = Some(0x1001);
    let img_t32 = Image::from_binary(Arc::from(thumb), bin_t32);
    let prog_t32 = Program::analyze(&img_t32, &Limits::default(), &NoControl).expect("analyze T32");
    let out_t32 = lift(&img_t32, &prog_t32, prog_t32.functions.get(&0x1000).expect("T32 entry"), &|_| None);

    assert!(out_t32.text.contains("r2 = *(uint32_t*)0x100c;"), "T32 target load must remain visible:\n{}", out_t32.text);
    assert!(out_t32.text.contains("(*r2)(7);"), "T32 blx r2 must pass only r0:\n{}", out_t32.text);
}

/// Win64 (`call rcx`) and SysV x86-64 (`call rcx`) indirect calls must keep `rcx = ...;` intact
/// instead of inlining the function pointer as a call argument.
#[test]
fn win64_and_sysv_indirect_register_calls_preserve_callee_assignment() {
    // Win64 PE: sub rsp, 0x28; mov rcx, [rax]; call rcx; add rsp, 0x28; ret
    let mut pe = tk::pe64();
    let pe_code = [
        0x48, 0x83, 0xec, 0x28, // sub rsp, 0x28
        0x48, 0x8b, 0x08, // mov rcx, [rax]
        0xff, 0xd1, // call rcx
        0x48, 0x83, 0xc4, 0x28, // add rsp, 0x28
        0xc3, // ret
    ];
    pe[0x400..0x400 + pe_code.len()].copy_from_slice(&pe_code);
    let img_pe = Image::parse(Arc::from(pe)).expect("parse PE64");
    let prog_pe = Program::analyze(&img_pe, &Limits::default(), &NoControl).expect("analyze PE64");
    let main_pe = prog_pe.functions.get(&(tk::PE_BASE + tk::PE_TEXT_RVA)).expect("PE main");
    let out_pe = lift(&img_pe, &prog_pe, main_pe, &|_| None);

    assert!(out_pe.text.contains("rcx = *(uint64_t*)(rax);"), "Win64 callee load must remain visible:\n{}", out_pe.text);
    assert!(out_pe.text.contains("(*rcx)();"), "Win64 0-arg indirect call must not pass its own callee ptr:\n{}", out_pe.text);

    // SysV x86-64 at 0x1000:
    //   0x1000: mov edi, 10               (bf 0a 00 00 00)
    //   0x1005: mov rcx, [rip + 0xff4]    (48 8b 0d f4 0f 00 00 -> 0x2000)
    //   0x100c: call rcx                  (ff d1)
    //   0x100e: ret                       (c3)
    let mut sysv = vec![0xccu8; 0x20];
    sysv[..15].copy_from_slice(&[0xbf, 0x0a, 0x00, 0x00, 0x00, 0x48, 0x8b, 0x0d, 0xf4, 0x0f, 0x00, 0x00, 0xff, 0xd1, 0xc3]);
    let mut bin_sysv = Binary::raw(Arch::X86_64, 0x1000, sysv.len() as u64, 0);
    bin_sysv.entry = Some(0x1000);
    let img_sysv = Image::from_binary(Arc::from(sysv), bin_sysv);
    let prog_sysv = Program::analyze(&img_sysv, &Limits::default(), &NoControl).expect("analyze SysV");
    let out_sysv = lift(&img_sysv, &prog_sysv, prog_sysv.functions.get(&0x1000).expect("SysV entry"), &|_| None);

    assert!(out_sysv.text.contains("rcx = *(uint64_t*)0x2000;"), "SysV callee load must remain visible:\n{}", out_sysv.text);
    assert!(out_sysv.text.contains("(*rcx)(10);"), "SysV call rcx must not invent rsi/rdx/rcx args:\n{}", out_sysv.text);
}

/// Memory-indirect calls (`call [rdi + 0x10]` and `call [rax + rcx * 8]`) must preserve both base
/// and index register assignments referenced by the call target expression instead of deleting them
/// or folding the index register into synthetic gap arguments.
#[test]
fn x64_memory_indirect_call_preserves_base_and_index_assignments() {
    // Dispatch-table base register (`rdi`) with a second argument (`esi = 42`):
    // 0x1000: mov rdi, [rbp - 8] (48 8b 7d f8); mov esi, 42 (be 2a 00 00 00);
    //         call qword ptr [rdi + 0x10] (ff 57 10); ret (c3)
    let code = vec![0x48, 0x8b, 0x7d, 0xf8, 0xbe, 0x2a, 0x00, 0x00, 0x00, 0xff, 0x57, 0x10, 0xc3];
    let mut bin = Binary::raw(Arch::X86_64, 0x1000, code.len() as u64, 0);
    bin.entry = Some(0x1000);
    let img = Image::from_binary(Arc::from(code), bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze base-indirect call");
    let out = lift(&img, &prog, prog.functions.get(&0x1000).expect("entry"), &|_| None);

    assert!(out.text.contains("rdi = var_8;"), "dispatch-table base assignment must not be deleted:\n{}", out.text);
    assert!(out.text.contains("(**(uint64_t*)(rdi + 16))(rdi, 0x2a);"), "dispatch-table call must pass rdi and 0x2a:\n{}", out.text);

    // Scaled index register (`rcx`, 4th SysV argument register):
    // 0x1000: mov ecx, [rbp - 4] (8b 4d fc); call qword ptr [rax + rcx*8] (ff 14 c8); ret (c3)
    let indexed = vec![0x8b, 0x4d, 0xfc, 0xff, 0x14, 0xc8, 0xc3];
    let mut bin_idx = Binary::raw(Arch::X86_64, 0x1000, indexed.len() as u64, 0);
    bin_idx.entry = Some(0x1000);
    let img_idx = Image::from_binary(Arc::from(indexed), bin_idx);
    let prog_idx = Program::analyze(&img_idx, &Limits::default(), &NoControl).expect("analyze indexed indirect call");
    let out_idx = lift(&img_idx, &prog_idx, prog_idx.functions.get(&0x1000).expect("entry"), &|_| None);

    assert!(out_idx.text.contains("ecx = var_4;"), "index register assignment must remain visible:\n{}", out_idx.text);
    assert!(out_idx.text.contains("(**(uint64_t*)(rax + rcx * 8))();"), "indexed indirect call must not invent rdi/rsi/rdx/rcx args:\n{}", out_idx.text);
}

/// A dispatch-table base register that is the only argument register set still passes as the
/// call's one argument, with its assignment kept, as it does when a second argument follows.
#[test]
fn x64_memory_indirect_call_passes_a_lone_base_register_argument() {
    // 0x1000: mov rdi, [rbp - 8] (48 8b 7d f8); call qword ptr [rdi + 0x10] (ff 57 10); ret (c3)
    let code = vec![0x48, 0x8b, 0x7d, 0xf8, 0xff, 0x57, 0x10, 0xc3];
    let mut bin = Binary::raw(Arch::X86_64, 0x1000, code.len() as u64, 0);
    bin.entry = Some(0x1000);
    let img = Image::from_binary(Arc::from(code), bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze one-argument base-indirect call");
    let out = lift(&img, &prog, prog.functions.get(&0x1000).expect("entry"), &|_| None);

    assert!(out.text.contains("rdi = var_8;"), "dispatch-table base assignment must not be deleted:\n{}", out.text);
    assert!(out.text.contains("(**(uint64_t*)(rdi + 16))(rdi);"), "one-argument dispatch-table call must pass rdi:\n{}", out.text);
}

/// `adrp x3, 0x2000; ldr x3, [x3, #0x10]; mov w0, #9; blr x3; ret` at 0x1000.
fn aarch64_slot_call_bytes() -> Vec<u8> {
    let words_slot = [0xb000_0003u32, 0xf940_0863, 0x5280_0120, 0xd63f_0060, 0xd65f_03c0];
    let mut bytes_slot = vec![0u8; 0x1020];
    for (idx, w) in words_slot.into_iter().enumerate() {
        bytes_slot[idx * 4..(idx + 1) * 4].copy_from_slice(&w.to_le_bytes());
    }
    bytes_slot
}

/// The pseudo-C of the function at `entry` in an AArch64 image.
fn aarch64_lift(bin: Binary, bytes: Vec<u8>, entry: u64) -> String {
    let img = Image::from_binary(Arc::from(bytes), bin);
    let prog = Program::analyze(&img, &Limits { scan_prologues: false, ..Limits::default() }, &NoControl).expect("analyze A64 image");
    lift(&img, &prog, prog.functions.get(&entry).expect("function"), &|_| None).text
}

/// `blraa x1, x2` at 0x1000 (after `mov w0, #5; mov x1, x19; mov x2, x20`) and a direct
/// `direct_caller` at 0x1030 that sets w0 and w1 before `bl 0x1020`.
fn aarch64_pac_and_direct_image() -> (Binary, Vec<u8>) {
    let mut words_pac = [0xd503_201fu32; 20];
    words_pac[0] = 0x5280_00a0; // 0x1000: mov w0, #5
    words_pac[1] = 0xaa13_03e1; // 0x1004: mov x1, x19
    words_pac[2] = 0xaa14_03e2; // 0x1008: mov x2, x20
    words_pac[3] = 0xd73f_0822; // 0x100c: blraa x1, x2
    words_pac[4] = 0xd65f_03c0; // 0x1010: ret
    words_pac[8] = 0xd65f_03c0; // 0x1020: ret (direct callee)
    words_pac[12] = 0x5280_00a0; // 0x1030: mov w0, #5
    words_pac[13] = 0x5280_00c1; // 0x1034: mov w1, #6
    words_pac[14] = 0x97ff_fffa; // 0x1038: bl 0x1020 (-6 insns)
    words_pac[15] = 0xd65f_03c0; // 0x103c: ret
    let bytes_pac: Vec<u8> = words_pac.into_iter().flat_map(u32::to_le_bytes).collect();
    let mut bin_pac = Binary::raw(Arch::Aarch64, 0x1000, bytes_pac.len() as u64, 0);
    bin_pac.entry = Some(0x1000);
    bin_pac.symbols.push(Symbol {
        name: "direct_caller".into(),
        demangled: None,
        addr: 0x1030,
        size: 16,
        kind: SymbolKind::Function,
        global: true,
        weak: false,
        defined: true,
        dynamic: false,
    });
    (bin_pac, bytes_pac)
}

/// AArch64 `blr x3` through a non-import global pointer slot (`g_callback`) must not fold the
/// callee register into the argument list.
#[test]
fn aarch64_global_slot_call_excludes_the_callee_register() {
    let bytes_slot = aarch64_slot_call_bytes();
    let mut bin_slot = Binary::raw(Arch::Aarch64, 0x1000, bytes_slot.len() as u64, 0);
    bin_slot.entry = Some(0x1000);
    bin_slot.symbols.push(Symbol {
        name: "g_callback".into(),
        demangled: None,
        addr: 0x2010,
        size: 8,
        kind: SymbolKind::Data,
        global: true,
        weak: false,
        defined: true,
        dynamic: false,
    });
    let out_slot = aarch64_lift(bin_slot, bytes_slot, 0x1000);
    assert!(out_slot.contains("x3 = g_callback;"), "non-import slot load must remain visible:\n{out_slot}");
    assert!(out_slot.contains("g_callback(9);"), "A64 global slot call must pass only w0:\n{out_slot}");
    assert!(!out_slot.contains("g_callback(9, x1"), "must not pass x1..x3:\n{out_slot}");
}

/// Control: when the slot is an import slot (`custom_imp`), `ldr x3` is still elided cleanly.
#[test]
fn aarch64_import_slot_call_elides_the_got_load() {
    let bytes_slot = aarch64_slot_call_bytes();
    let mut bin_imp = Binary::raw(Arch::Aarch64, 0x1000, bytes_slot.len() as u64, 0);
    bin_imp.entry = Some(0x1000);
    bin_imp.import_slots.push(ImportSlot { addr: 0x2010, name: "custom_imp".into(), library: Some("libc.so".into()), kind: "got" });
    let out_imp = aarch64_lift(bin_imp, bytes_slot, 0x1000);
    assert!(!out_imp.contains("x3 ="), "import-slot load must remain elided:\n{out_imp}");
    assert!(out_imp.contains("custom_imp(9);"), "import-slot call must pass w0:\n{out_imp}");
}

/// `blraa x1, x2` must not fold the callee register or pointer-authentication modifier into the
/// argument list.
#[test]
fn aarch64_authenticated_call_excludes_the_callee_and_modifier() {
    let (bin_pac, bytes_pac) = aarch64_pac_and_direct_image();
    let out_pac = aarch64_lift(bin_pac, bytes_pac, 0x1000);
    assert!(out_pac.contains("x1 = x19;"), "PAC target assignment must remain visible:\n{out_pac}");
    assert!(out_pac.contains("x2 = x20;"), "PAC modifier assignment must remain visible:\n{out_pac}");
    assert!(out_pac.contains("(*x1)(5);"), "blraa x1, x2 must pass only w0:\n{out_pac}");
}

/// Control: an ordinary direct `bl` still folds its argument registers.
#[test]
fn aarch64_direct_call_folds_its_arguments() {
    let (bin_pac, bytes_pac) = aarch64_pac_and_direct_image();
    let out_direct = aarch64_lift(bin_pac, bytes_pac, 0x1030);
    assert!(out_direct.contains("sub_1020(5, 6);"), "direct bl control must still fold w0 and w1:\n{out_direct}");
}

/// A base register assigned in the block stays the call's argument after an earlier read of it
/// (here a spill to the frame), and its assignment stays visible.
#[test]
fn x64_memory_indirect_call_keeps_a_base_argument_read_before_the_call() {
    // 0x1000: mov rdi, [rbp - 8] (48 8b 7d f8); mov [rbp - 16], rdi (48 89 7d f0);
    //         call qword ptr [rdi + 0x10] (ff 57 10); ret (c3)
    let code = vec![0x48, 0x8b, 0x7d, 0xf8, 0x48, 0x89, 0x7d, 0xf0, 0xff, 0x57, 0x10, 0xc3];
    let mut bin = Binary::raw(Arch::X86_64, 0x1000, code.len() as u64, 0);
    bin.entry = Some(0x1000);
    let img = Image::from_binary(Arc::from(code), bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze spilled base-indirect call");
    let out = lift(&img, &prog, prog.functions.get(&0x1000).expect("entry"), &|_| None);

    assert!(out.text.contains("rdi = var_8;"), "base assignment must stay visible:\n{}", out.text);
    assert!(out.text.contains("(**(uint64_t*)(rdi + 16))(rdi);"), "the spilled base must still be passed:\n{}", out.text);
}
