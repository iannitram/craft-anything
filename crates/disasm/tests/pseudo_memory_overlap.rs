//! Regressions for argument loads and outgoing stack arguments that pseudo-C folds into a call:
//! a memory write that may change the bytes they read keeps them in place.

use craft_anything_disasm::pseudo::lift;
use craft_anything_disasm::{Image, Limits, NoControl, Program};
use craft_anything_formats::{Arch, Binary};
use craft_anything_testkit as tk;
use std::sync::Arc;

/// The pseudo-C of the function at 0x1000 in a raw image.
fn lift_raw(arch: Arch, code: Vec<u8>) -> String {
    let mut bin = Binary::raw(arch, 0x1000, code.len() as u64, 0);
    bin.entry = Some(0x1000);
    let img = Image::from_binary(Arc::from(code), bin);
    let prog = Program::analyze(&img, &Limits { scan_prologues: false, ..Limits::default() }, &NoControl).expect("analyze");
    lift(&img, &prog, prog.functions.get(&0x1000).expect("function at 0x1000"), &|_| None).text
}

/// The trimmed line of `text` that calls `callee`.
fn call_line(text: &str, callee: &str) -> String {
    text.lines().find(|l| l.contains(&format!("{callee}("))).unwrap_or_default().trim().to_string()
}

/// AArch64 `ldr x0, [x19, #8]; <store>; bl 0x1010`: `stp x1, x2, [x19]` writes 16 bytes and so
/// reaches the load's, so the load stays before it; `str x1, [x19]` writes 8 bytes that end where
/// the load starts, so the load still folds into the call. The decoder sizes neither access.
#[test]
fn a_pair_store_keeps_an_a64_argument_load_and_a_disjoint_store_does_not() {
    for (store, call) in [(0xa900_0a61u32, "sub_1010(x0);"), (0xa800_0a61, "sub_1010(x0);"), (0xf900_0261, "sub_1010(*(void*)(x19 + 8));")] {
        let words = [0xf940_0660u32, store, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0];
        let text = lift_raw(Arch::Aarch64, words.iter().flat_map(|w| w.to_le_bytes()).collect());
        assert_eq!(call_line(&text, "sub_1010"), call, "{store:#x}:\n{text}");
        assert_eq!(text.contains("x0 = *(void*)(x19 + 8);"), call == "sub_1010(x0);", "{store:#x}:\n{text}");
    }
}

/// AArch64 `ldr x0, 0x1018` (a literal, named by address rather than as a memory operand); then
/// `str x1, [x2]`, which may write the literal, keeps the load in place, while `str x1, [sp, #8]`,
/// a frame slot, lets it fold.
#[test]
fn an_a64_literal_load_stays_before_a_store_that_may_write_it() {
    for (store, kept) in [(0xf900_0041u32, true), (0xf900_07e1, false)] {
        let words = [0x5800_00c0u32, store, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0, 0xd503_201f, 0, 0];
        let text = lift_raw(Arch::Aarch64, words.iter().flat_map(|w| w.to_le_bytes()).collect());
        assert_eq!(call_line(&text, "sub_1010"), if kept { "sub_1010(x0);" } else { "sub_1010(*(uint64_t*)0x1018);" }, "{store:#x}:\n{text}");
        assert_eq!(text.lines().any(|l| l.trim_start().starts_with("x0 = ")), kept, "{store:#x}:\n{text}");
    }
}

/// An A64 frame-pointer register can hold an absolute address or an unknown pointer.
#[test]
fn a_frame_pointer_store_uses_its_address() {
    for (setup, call) in [
        (0x1000_00ddu32, "sub_1014(x0);"),              // adr x29, 0x1018 (the literal)
        (0x1000_011d, "sub_1014(*(uint64_t*)0x1018);"), // adr x29, 0x1020
        (0x9100_03fd, "sub_1014(*(uint64_t*)0x1018);"), // mov x29, sp
        (0xd503_201f, "sub_1014(x0);"),                 // nop: x29 has unknown provenance
    ] {
        let words = [setup, 0x5800_00a0, 0xf900_03a1, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0, 0, 0, 0, 0];
        let text = lift_raw(Arch::Aarch64, words.iter().flat_map(|w| w.to_le_bytes()).collect());
        assert_eq!(call_line(&text, "sub_1014"), call, "{setup:#x}:\n{text}");
    }
}

/// The status register of an exclusive store does not describe its memory width.
#[test]
fn exclusive_store_width_comes_from_the_data() {
    for (load, store, keep) in [
        (0xf840_4260u32, 0xc803_7e61, true), // ldur x0, [x19,#4]; stxr w3,x1,[x19]
        (0xf940_0660, 0xc803_fe61, false),   // offset 8 is past a single stlxr
        (0xf840_c260, 0xc823_0a61, true),    // byte 12 is beyond status + first data register but inside the pair
        (0xf940_0660, 0xc823_0a61, true),    // stxp w3,x1,x2,[x19]
        (0xf940_0660, 0xc823_8a61, true),    // stlxp w3,x1,x2,[x19]
    ] {
        let words = [load, store, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0];
        let text = lift_raw(Arch::Aarch64, words.iter().flat_map(|w| w.to_le_bytes()).collect());
        assert_eq!(call_line(&text, "sub_1010"), if keep { "sub_1010(x0);" } else { "sub_1010(*(void*)(x19 + 8));" }, "{text}");
    }
}

/// MOVDIR64B writes through its register operand and reads the explicit memory operand.
#[test]
fn direct_store_writes_its_register_destination() {
    for (offset, call) in [(56u8, "sub_1020(rdi);"), (64, "sub_1020(*(uint64_t*)(rax + 0x40));")] {
        let mut code = vec![0x48, 0x8b, 0x78, offset, 0x66, 0x0f, 0x38, 0xf8, 0x40, 0x40, 0xe8, 0x11, 0, 0, 0, 0xc3];
        code.resize(0x20, 0xcc);
        code.push(0xc3);
        let text = lift_raw(Arch::X86_64, code);
        assert_eq!(call_line(&text, "sub_1020"), call, "{text}");
    }
}

/// A register bit index can move a memory write below the operand's nominal address.
#[test]
fn register_bit_writes_keep_an_earlier_load() {
    for (prefix, opcode) in [(0x90, 0xab), (0x90, 0xb3), (0x90, 0xbb), (0xf0, 0xab)] {
        let mut code = vec![0x48, 0x8b, 0x78, 0xf8, prefix, 0x48, 0x0f, opcode, 0x10, 0xe8, 0x12, 0, 0, 0, 0xc3];
        code.resize(0x20, 0xcc);
        code.push(0xc3);
        let text = lift_raw(Arch::X86_64, code);
        assert_eq!(call_line(&text, "sub_1020"), "sub_1020(rdi);", "{opcode:#x}:\n{text}");
        assert!(text.contains("rdi = *(uint64_t*)(rax - 8);"), "{text}");
    }
}

/// x86-64 `mov rdi, <load>; <store>; call 0x1020` (each padded with `nop`s): a load through `rbp`
/// with an index may read what a RIP-relative store writes, and `fs:[rax]` what `[rax+8]` writes;
/// an unindexed frame slot and an unsegmented `[rax]` still fold.
#[test]
fn indexed_and_segment_loads_stay_before_a_store() {
    let rip_store = [0x48, 0x89, 0x15, 0x00, 0x01, 0x00, 0x00]; // mov [rip+0x100], rdx
    let rax8_store = [0x48, 0x89, 0x50, 0x08, 0x90, 0x90, 0x90]; // mov [rax+8], rdx
    for (load, store, call) in [
        ([0x48u8, 0x8b, 0x7c, 0x05, 0x00], rip_store, "sub_1020(rdi);"), // mov rdi, [rbp+rax]
        ([0x48, 0x8b, 0x7d, 0x00, 0x90], rip_store, "sub_1020(arg_0);"), // mov rdi, [rbp]
        ([0x64, 0x48, 0x8b, 0x38, 0x90], rax8_store, "sub_1020(rdi);"),  // mov rdi, fs:[rax]
        ([0x48, 0x8b, 0x38, 0x90, 0x90], rax8_store, "sub_1020(*(uint64_t*)(rax));"), // mov rdi, [rax]
    ] {
        let mut code = [&[0x48, 0x89, 0xe5][..], &load, &store, &[0xe8, 0x0c, 0x00, 0x00, 0x00, 0xc3]].concat();
        code.resize(0x20, 0xcc);
        code.push(0xc3);
        let text = lift_raw(Arch::X86_64, code);
        assert_eq!(call_line(&text, "sub_1020"), call, "{load:x?}:\n{text}");
        assert_eq!(text.contains("rdi = "), call == "sub_1020(rdi);", "{load:x?}:\n{text}");
    }
}

/// A conditional frame-pointer assignment can leave an arbitrary old pointer in rbp.
#[test]
fn a_conditional_move_does_not_establish_a_frame() {
    let mut code = vec![0x48, 0x0f, 0x45, 0xec, 0x48, 0x8b, 0x7d, 0, 0x48, 0x89, 0x15, 0, 1, 0, 0, 0xe8, 12, 0, 0, 0, 0xc3];
    code.resize(0x20, 0xcc);
    code.push(0xc3);
    let text = lift_raw(Arch::X86_64, code);
    assert_eq!(call_line(&text, "sub_1020"), "sub_1020(rdi);", "{text}");
}

/// Saving frame registers into ordinary memory is visible even in the entry block.
#[test]
fn a_pair_store_through_a_general_register_is_visible() {
    let words = [0xa800_7a7du32, 0xd65f_03c0]; // stnp x29, x30, [x19]; ret
    let text = lift_raw(Arch::Aarch64, words.iter().flat_map(|w| w.to_le_bytes()).collect());
    assert!(text.contains("[0] = x29;") && text.contains("[1] = x30;"), "{text}");
}

/// Win64: `mov [rsp+0x20], rax; inc qword [rsp+0x20]` before an import call passes the slot as it
/// is at the call and keeps the store that set it.
#[test]
fn a_write_to_an_outgoing_stack_slot_stops_it_folding() {
    for (overwrite, statement, call) in [
        ([0x48, 0xff, 0x44, 0x24, 0x20], "stack_20++;", "puts(1, 2, 3, 4, stack_20);"),
        ([0xc6, 0x44, 0x24, 0x20, 1], "stack_20 = 1;", "puts(1, 2, 3, 4, stack_20);"),
        ([0x48, 0x89, 0x5c, 0x24, 0x20], "", "puts(1, 2, 3, 4, rbx);"),
    ] {
        let mut pe = tk::pe64();
        let mut code: [u8; 47] = [
            0x48, 0x83, 0xec, 0x68, // sub rsp, 0x68
            0x48, 0x89, 0x44, 0x24, 0x20, // mov [rsp+0x20], rax
            0x48, 0xff, 0x44, 0x24, 0x20, // inc qword [rsp+0x20]
            0x41, 0xb9, 0x04, 0x00, 0x00, 0x00, // mov r9d, 4
            0x41, 0xb8, 0x03, 0x00, 0x00, 0x00, // mov r8d, 3
            0xba, 0x02, 0x00, 0x00, 0x00, // mov edx, 2
            0xb9, 0x01, 0x00, 0x00, 0x00, // mov ecx, 1
            0xff, 0x15, 0x36, 0x11, 0x00, 0x00, // call [rip+0x1136] (the puts IAT slot)
            0x48, 0x83, 0xc4, 0x68, // add rsp, 0x68
            0xc3, // ret
        ];
        code[9..14].copy_from_slice(&overwrite);
        pe[0x400..0x400 + code.len()].copy_from_slice(&code);
        let img = Image::parse(Arc::from(pe)).expect("parse PE64");
        let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze PE64");
        let text = lift(&img, &prog, prog.functions.get(&(tk::PE_BASE + tk::PE_TEXT_RVA)).expect("main"), &|_| None).text;
        assert_eq!(call_line(&text, "puts"), call, "{text}");
        assert!(text.contains("stack_20 = rax;") && text.contains(statement), "{text}");
    }
}

/// Lift a little-endian AArch64 instruction sequence at 0x1000.
fn lift_words(words: &[u32]) -> String {
    lift_raw(Arch::Aarch64, words.iter().flat_map(|word| word.to_le_bytes()).collect())
}

/// A vector pair store writes both vector values and can overwrite the upper half of either one.
#[test]
fn simd_non_temporal_store_preserves_its_operands() {
    // ldr x0, [x19,#24]; stnp q0, q1, [x19]; bl 0x1010
    let text = lift_words(&[0xf940_0e60, 0xac00_0660, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0]);
    assert!(text.contains("stnp q0, q1, [x19]"), "{text}");
    assert_eq!(call_line(&text, "sub_1010"), "sub_1010(x0);", "{text}");
}

/// A register post-index updates the base after the vector load. Plain addressing leaves it intact.
#[test]
fn post_index_retires_the_resolved_base() {
    for (ld1, arg) in [(0x4cd4_7260, "x0"), (0x4c40_7260, "*(uint64_t*)0x1030")] {
        // adr x19,0x1020; ld1 {v0.16b},[x19]{,x20}; ldr x0,0x1030; str x1,[x19]
        let text = lift_words(&[0x1000_0113, ld1, 0x5800_0140, 0xf900_0261, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0]);
        assert_eq!(call_line(&text, "sub_1018"), format!("sub_1018({arg});"), "{text}");
    }
}

/// A redirected stack pointer can address the same object as a resolved store, including in a successor.
#[test]
fn redirected_stack_can_alias_a_global() {
    for (setup, arg) in [(0x9100_027f, "x0"), (0x4cd4_73e0, "x0"), (0x9100_43ff, "stack_0"), (0xf841_07ea, "stack_0")] {
        // Set SP from x19 or post-index it by x20. The controls move SP by a fixed 16 bytes.
        // ldr x0,[sp]; str x1,[x19]; bl 0x1020
        let text = lift_words(&[0x1000_0213, setup, 0x1400_0001, 0x1000_01b3, 0xf940_03e0, 0xf900_0261, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0]);
        assert_eq!(call_line(&text, "sub_1020"), format!("sub_1020({arg});"), "{text}");
    }
}

/// Address-size override zero-extends ESP. A full RSP destination still denotes a stack access.
#[test]
fn direct_store_with_a_truncated_pointer_may_reach_a_global() {
    for (prefix, arg) in [(0x67, "rdi"), (0x48, "*(uint32_t*)0x1040")] {
        // mov edi,[rip+0x3a]; movdir64b esp,[esi] (control: rsp,[rsi]); call 0x1020
        let mut code = vec![0x8b, 0x3d, 0x3a, 0, 0, 0, 0x66, prefix, 0x0f, 0x38, 0xf8, 0x26, 0xe8, 0x0f, 0, 0, 0, 0xc3];
        code.resize(0x20, 0xcc);
        code.push(0xc3);
        let text = lift_raw(Arch::X86_64, code);
        assert_eq!(call_line(&text, "sub_1020"), format!("sub_1020({arg});"), "{text}");
    }
}

/// The folded literal reads exactly the low word. An eight-byte load overlaps the following store.
#[test]
fn literal_width_matches_the_folded_read() {
    for (load, arg) in [(0x1800_01e0, "*(uint32_t*)0x1040"), (0x5800_01e0, "x0")] {
        let text = lift_words(&[0x1000_0213, load, 0xb900_0661, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0, 0, 0]);
        assert_eq!(call_line(&text, "sub_1014"), format!("sub_1014({arg});"), "{text}");
    }
}

/// A stack-relative STNP is an ordinary write even when its sources are the frame and link registers.
#[test]
fn non_temporal_stack_pair_is_visible() {
    let text = lift_words(&[0xa802_7bfd, 0xf940_17e0, 0xd65f_03c0]);
    assert!(text.contains("[0] = x29;") && text.contains("[1] = x30;"), "{text}");
    let prologue = lift_words(&[0xa9bf_7bfd, 0xd65f_03c0]);
    assert!(!prologue.contains("[0] = x29;"), "{prologue}");
}

/// A resolved pair store covers both registers. A load at the first byte past the pair still folds.
#[test]
fn resolved_pair_bounds_allow_adjacent_loads() {
    for (load, arg) in [(0xf940_0a60, "*(uint64_t*)0x1030"), (0xf940_0660, "x0")] {
        let text = lift_words(&[0x1000_0113, load, 0xa900_0a61, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0]);
        assert_eq!(call_line(&text, "sub_1014"), format!("sub_1014({arg});"), "{text}");
    }
}

/// Resolved scalar loads retain their width and extension when a disjoint pair store lets them fold.
#[test]
fn resolved_unscaled_and_narrow_loads_keep_their_type() {
    for (load, pair, arg) in [
        (0xb840_0260, 0x2900_8a61, "*(uint32_t*)0x1040"),         // ldur w0; store at +4
        (0x3940_0260, 0x2900_8a61, "*(uint8_t*)0x1040"),          // ldrb w0
        (0x7840_0260, 0x2900_8a61, "*(uint16_t*)0x1040"),         // ldurh w0
        (0x38c0_0260, 0x2900_8a61, "(uint32_t)*(int8_t*)0x1040"), // ldursb w0
        (0x7980_0260, 0x2901_0a61, "*(int16_t*)0x1040"),          // ldrsh x0; store at +8
        (0xb980_0260, 0x2900_8a61, "*(int32_t*)0x1040"),          // ldrsw x0
    ] {
        let text = lift_words(&[0x1000_0213, load, pair, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0]);
        assert_eq!(call_line(&text, "sub_1014"), format!("sub_1014({arg});"), "{text}");
    }
}

/// A pair writes through the old base and then updates it. The memory address is not the register result.
#[test]
fn pair_writeback_keeps_its_address_separate_from_register_values() {
    use craft_anything_disasm::{ArmConsts, Mode, Op, decode};
    let mut known = ArmConsts::default();
    let mut adr = decode(Mode::A64, &0x1000_0200u32.to_le_bytes(), 0x1000, false).expect("adr x0,0x1040");
    known.apply(&mut adr);
    let mut store = decode(Mode::A64, &0xa881_0801u32.to_le_bytes(), 0x1004, false).expect("stp x1,x2,[x0],#16");
    known.apply(&mut store);
    assert!(store.refs.is_empty(), "{store:?}");
    assert!(matches!(store.ops.get(2), Some(Op::Mem(m)) if m.addr == Some(0x1040)));
    assert_eq!(known.value("x0"), None);
}

/// Ordinary frame-register stores remain visible on a relocated stack and in offset-only addressing.
#[test]
fn stack_pair_suppression_requires_a_frame_save() {
    let relocated = lift_words(&[0x1000_0213, 0x9100_027f, 0xa9bf_7bfd, 0xd65f_03c0]);
    assert!(relocated.contains("stp x29, x30, [sp, #-0x10]!"), "{relocated}");
    let ordinary = lift_words(&[0xa900_7bfd, 0xd65f_03c0]);
    assert!(ordinary.contains("[0] = x29;") && ordinary.contains("[1] = x30;"), "{ordinary}");
    let duplicate = lift_words(&[0xa9bf_77fd, 0xd65f_03c0]); // stp x29,x29,[sp,#-16]!
    assert!(duplicate.contains("stp x29, x29, [sp, #-0x10]!"), "{duplicate}");
}

/// A visible pair with writeback keeps the base update and its order in the assembly.
#[test]
fn visible_pair_preserves_pre_and_post_index() {
    for (pair, addressing) in [(0xa981_7a7d, "[x19, #0x10]!"), (0xa881_7a7d, "[x19], #0x10")] {
        let text = lift_words(&[0x1000_0213, pair, 0xaa13_03e0, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0]);
        assert!(text.contains(&format!("stp x29, x30, {addressing}")), "{text}");
        assert_eq!(call_line(&text, "sub_1014"), "sub_1014(x19);", "{text}");
    }
}

/// Offset-only store pairs index a pointer to one element for either register width.
#[test]
fn pair_stores_have_typed_destinations() {
    for (pair, ty, reg) in [(0xa900_0a61, "uint64_t", "x"), (0x2900_0a61, "uint32_t", "w")] {
        for (setup, base) in [(0xd503_201f, "x19"), (0x1000_0213, "0x1040")] {
            let text = lift_words(&[setup, pair, 0xd65f_03c0]);
            for (ix, n) in [(0, 1), (1, 2)] {
                assert!(text.contains(&format!("(({ty}*)({base}))[{ix}] = {reg}{n};")), "{text}");
            }
        }
    }
}

/// An x86-64 stack pivot can put a full-width direct store at the same address as a global load.
#[test]
fn redirected_rsp_can_alias_a_global() {
    // lea rsp,[rip+0x39]; mov edi,[rip+0x33]; movdir64b rsp,[rsi]; call 0x1020
    let mut code = vec![0x48, 0x8d, 0x25, 0x39, 0, 0, 0, 0x8b, 0x3d, 0x33, 0, 0, 0, 0x66, 0x48, 0x0f, 0x38, 0xf8, 0x26, 0xe8, 0x08, 0, 0, 0, 0xc3];
    code.resize(0x20, 0xcc);
    code.push(0xc3);
    for (setup, arg) in [
        (code[..7].to_vec(), "rdi"),
        (vec![0x5c, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90], "rdi"),
        (vec![0x58, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90], "*(uint32_t*)0x1040"),
    ] {
        code[..7].copy_from_slice(&setup);
        let text = lift_raw(Arch::X86_64, code.clone());
        assert_eq!(call_line(&text, "sub_1020"), format!("sub_1020({arg});"), "{text}");
    }
}

/// A Win64 stack argument built from two dword stores must retain both stores in either order.
#[test]
fn split_stack_argument_preserves_both_halves() {
    for (first, second) in [(0x24, 0x20), (0x20, 0x24)] {
        let mut pe = tk::pe64();
        let code = [0x89, 0x44, 0x24, first, 0x89, 0x5c, 0x24, second, 0xff, 0x15, 0x52, 0x11, 0, 0, 0xc3];
        pe[0x400..0x400 + code.len()].copy_from_slice(&code);
        let img = Image::parse(Arc::from(pe)).expect("parse");
        let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze");
        let text = lift(&img, &prog, prog.functions.get(&(tk::PE_BASE + tk::PE_TEXT_RVA)).expect("entry"), &|_| None).text;
        assert!(text.contains(&format!("stack_{first:x} = eax;")) && text.contains(&format!("stack_{second:x} = ebx;")), "{text}");
        assert_eq!(call_line(&text, "puts"), "puts(rcx, rdx, r8, r9, stack_20);", "{text}");
    }
}

/// String substitution would turn a writable pair destination into a different object.
#[test]
fn a_store_pair_keeps_the_address_of_writable_text() {
    let mut code: Vec<u8> = [0x1000_0213u32, 0xa900_0a61, 0xd65f_03c0].iter().flat_map(|x| x.to_le_bytes()).collect();
    code.resize(0x40, 0);
    code.extend_from_slice(b"writable bytes\0");
    let text = lift_raw(Arch::Aarch64, code);
    assert!(text.contains("((uint64_t*)(0x1040))[0] = x1;"), "{text}");
}

/// A folded byte load through an unresolved pointer retains the W-register sign extension.
#[test]
fn unresolved_signed_byte_retains_its_conversion() {
    // ldrsb w0,[x19]; strb w1,[x19,#1]; bl 0x1010
    let text = lift_words(&[0x39c0_0260, 0x3900_0661, 0x9400_0002, 0xd65f_03c0, 0xd65f_03c0]);
    assert_eq!(call_line(&text, "sub_1010"), "sub_1010((uint32_t)*(int8_t*)(x19));", "{text}");
}
