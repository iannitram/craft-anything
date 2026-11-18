//! Regressions for ARM32 and Thumb PC-relative literal pool loads (`ldr rt, [pc, #off]`),
//! PC-relative indirect jumps (`ldr pc, [pc, #off]` GOT/IAT thunks), and 32-bit pseudo-C
//! dereference formatting.

use craft_anything_disasm::{Flow, Image, Limits, Mode, NoControl, Op, Program, Xref, XrefKind, decode, pseudo};
use craft_anything_formats::{Arch, Binary, Format, ImportSlot, Section, Symbol, SymbolKind};
use std::sync::Arc;

/// Build an executable + data image (optional `.plt` at `0x1000..0x1000+plt_len`, `.text` after it,
/// `.got` at `0x2000`).
fn make_arm_image(entry: u64, plt_len: usize, code: &[u8], data: &[u8], import_slots: Vec<ImportSlot>, symbols: Vec<Symbol>) -> Image {
    let mut raw = vec![0u8; 0x200 + data.len()];
    let clen = code.len().min(0x100);
    raw[0x100..0x100 + clen].copy_from_slice(&code[..clen]);
    raw[0x200..0x200 + data.len()].copy_from_slice(data);

    let mut bin = Binary::raw(Arch::Arm, 0, 0, 0);
    bin.format = Format::Elf;
    bin.entry = Some(entry);
    bin.import_slots = import_slots;
    bin.symbols = symbols;
    let mut sections = Vec::new();
    if plt_len > 0 {
        sections.push(Section {
            name: ".plt".into(),
            segment: None,
            addr: 0x1000,
            size: plt_len as u64,
            file_offset: Some(0x100),
            file_size: plt_len as u64,
            read: true,
            write: false,
            exec: true,
            kind: "code",
            entropy: None,
            alloc: true,
        });
    }
    sections.push(Section {
        name: ".text".into(),
        segment: None,
        addr: 0x1000 + plt_len as u64,
        size: clen.saturating_sub(plt_len) as u64,
        file_offset: Some(0x100 + plt_len as u64),
        file_size: clen.saturating_sub(plt_len) as u64,
        read: true,
        write: false,
        exec: true,
        kind: "code",
        entropy: None,
        alloc: true,
    });
    sections.push(Section {
        name: ".got".into(),
        segment: None,
        addr: 0x2000,
        size: data.len() as u64,
        file_offset: Some(0x200),
        file_size: data.len() as u64,
        read: true,
        write: false,
        exec: false,
        kind: "data",
        entropy: None,
        alloc: true,
    });
    bin.sections = sections;
    Image::from_binary(Arc::from(raw), bin)
}

/// Every ARM32 and Thumb PC-relative literal load (`[pc]`, `[pc, #+imm]`, `[pc, #-imm]`, including
/// 16-bit and 32-bit Thumb instructions on odd halfword slots and byte loads) must record the
/// `Align(PC, 4) + disp` address reported by `llvm-objdump` in `Insn.refs`, and `ldr pc, [pc, #off]`
/// must populate `Flow::IndirectJump { slot: Some(addr) }`, while hints, invalid PC stores,
/// writeback/post-indexed forms, and register-indexed jump tables keep `refs` empty.
#[test]
fn arm_and_thumb_pc_literal_loads_record_llvm_objdump_targets() {
    let a32_cases: &[(u64, u32, u64, bool)] = &[
        (0x1000, 0xe59f_0000, 0x1008, false), // ldr r0, [pc]          -> 0x1008
        (0x1004, 0xe59f_1010, 0x101c, false), // ldr r1, [pc, #0x10]   -> 0x101c
        (0x1008, 0xe51f_2004, 0x100c, false), // ldr r2, [pc, #-0x4]   -> 0x100c
        (0x100c, 0xe59f_f008, 0x101c, true),  // ldr pc, [pc, #0x8]    -> 0x101c (IndirectJump)
        (0x1010, 0xe5df_0008, 0x1020, false), // ldrb r0, [pc, #0x8]   -> 0x1020
    ];
    for &(addr, word, expected, is_pc_jump) in a32_cases {
        for fmt in [false, true] {
            let insn = decode(Mode::A32, &word.to_le_bytes(), addr, fmt).expect("decode A32 literal load");
            assert_eq!(insn.refs, vec![expected], "A32 {addr:#x} fmt={fmt}: {insn:?}");
            assert!(!insn.reads.iter().any(|r| r == "r15"), "PC literal must not record dynamic r15 read: {insn:?}");
            if is_pc_jump {
                assert_eq!(insn.flow, Flow::IndirectJump { slot: Some(expected) }, "A32 {addr:#x}");
            }
        }
    }

    let thumb_cases: &[(u64, &[u8], u64, bool)] = &[
        (0x1016, &[0x00, 0x48], 0x1018, false),             // odd halfword 16-bit: ldr r0, [pc, #0] -> 0x1018
        (0x1018, &[0x04, 0x49], 0x102c, false),             // even word 16-bit:    ldr r1, [pc, #0x10] -> 0x102c
        (0x101a, &[0x5f, 0xf8, 0x04, 0x20], 0x1018, false), // odd halfword 32-bit: ldr.w r2, [pc, #-4] -> 0x1018
        (0x101e, &[0xdf, 0xf8, 0x0c, 0xf0], 0x102c, true),  // odd halfword 32-bit: ldr.w pc, [pc, #0xc] -> 0x102c
    ];
    for &(addr, raw, expected, is_pc_jump) in thumb_cases {
        for fmt in [false, true] {
            let insn = decode(Mode::T32, raw, addr, fmt).expect("decode Thumb literal load");
            assert_eq!(insn.refs, vec![expected], "T32 {addr:#x} fmt={fmt}: {insn:?}");
            assert!(!insn.reads.iter().any(|r| r == "r15"), "PC literal must not record dynamic r15 read: {insn:?}");
            if is_pc_jump {
                assert_eq!(insn.flow, Flow::IndirectJump { slot: Some(expected) }, "T32 {addr:#x}");
            }
        }
    }

    // Controls 1: non-PC indirect jump (`ldr pc, [r1, #4]`), pre-index writeback (`ldr pc, [pc, #4]!`),
    // post-indexed (`ldr pc, [pc], #4`), and register-indexed jump tables (`ldr pc, [pc, r1]` /
    // `ldr pc, [pc, r1, lsl #2]`) must keep `slot: None`, empty `refs`, and `MemOp.addr == None`.
    for word in [0xe591_f004u32, 0xe5bf_f004, 0xe49f_f004, 0xe79f_f001, 0xe79f_f101] {
        let dyn_jump = decode(Mode::A32, &word.to_le_bytes(), 0x1000, true).expect("decode non-literal PC jump");
        assert!(dyn_jump.refs.is_empty(), "{word:#010x}: {dyn_jump:?}");
        assert_eq!(dyn_jump.flow, Flow::IndirectJump { slot: None }, "{word:#010x}");
        assert!(matches!(dyn_jump.ops.get(1), Some(Op::Mem(m)) if m.addr.is_none()), "{word:#010x}: {:?}", dyn_jump.ops);
    }

    // Controls 2: Thumb unallocated memory hints (`nop.w`), preload hints (`pld [pc, #0x10]`), and
    // undefined Thumb PC-base stores (`str r0, [pc, #0x10]`) must never invent `Data` `refs`.
    for raw in [&[0xbfu8, 0xf9, 0x0c, 0xf0][..], &[0x9f, 0xf8, 0x10, 0xf0][..], &[0xcf, 0xf8, 0x10, 0x00][..]] {
        if let Some(insn) = decode(Mode::T32, raw, 0x1000, true) {
            assert!(insn.refs.is_empty(), "non-load Thumb encoding {raw:02x?} invented refs: {insn:?}");
            assert!(!insn.ops.iter().any(|op| matches!(op, Op::Mem(m) if m.addr.is_some())), "{insn:?}");
        }
    }
}

/// `Program::analyze` resolves `Data` and `Import` xrefs from ARM32/Thumb `[pc, #off]` loads and
/// identifies `ldr.w pc, [pc, #off]` GOT stubs as import thunks (including `noreturn` propagation).
#[test]
fn arm_and_thumb_pc_literal_xrefs_and_got_thunks_resolve_in_program() {
    let mut code = vec![0u8; 0x20];
    code[0x00..0x04].copy_from_slice(&[0xdf, 0xf8, 0xfc, 0xff]); // 0x1000 (.plt): ldr.w pc, [pc, #0xffc] -> 0x2000
    code[0x08..0x0a].copy_from_slice(&[0x80, 0xb5]); // 0x1008 (.text): push {r7, lr}
    code[0x0a..0x0e].copy_from_slice(&[0xdf, 0xf8, 0xf8, 0x0f]); // 0x100a: ldr.w r0, [pc, #0xff8] -> 0x2004
    code[0x0e..0x12].copy_from_slice(&[0xff, 0xf7, 0xf7, 0xff]); // 0x100e: bl 0x1000
    code[0x12..0x16].copy_from_slice(&[0x63, 0x20, 0x80, 0xbd]); // 0x1012: unreachable trailing code

    let slots = vec![ImportSlot { addr: 0x2000, name: "abort".into(), library: Some("libc.so".into()), kind: "got" }];
    let img = make_arm_image(0x1009, 8, &code, &[0u8; 8], slots, Vec::new());
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze Thumb GOT image");

    let entry_fn = prog.functions.get(&0x1008).expect("entry function at 0x1008");
    assert_eq!(entry_fn.calls.len(), 1);
    assert_eq!(entry_fn.calls[0].import.as_deref(), Some("abort"));
    assert!(!entry_fn.returns, "call to noreturn GOT thunk must stop fallthrough: {entry_fn:?}");
    assert_eq!(entry_fn.insn_count, 3, "trailing movs/pop must not be absorbed: {entry_fn:?}");

    let data_xrefs = prog.xrefs_to.get(&0x2004).cloned().unwrap_or_default();
    assert!(data_xrefs.contains(&Xref { from: 0x100a, to: 0x2004, kind: XrefKind::Data }), "expected Data xref from Thumb literal load: {data_xrefs:?}");

    let stub_fn = prog.functions.get(&0x1000).expect("abort GOT stub at 0x1000");
    assert_eq!(stub_fn.thunk_of.as_deref(), Some("abort"));
    let imp_xrefs = prog.xrefs_to.get(&0x2000).cloned().unwrap_or_default();
    assert!(imp_xrefs.contains(&Xref { from: 0x1000, to: 0x2000, kind: XrefKind::Import }), "expected Import xref from GOT stub: {imp_xrefs:?}");
}

/// Pseudo-C lifting for ARM32 and Thumb `[pc, #off]` literal loads keeps 32-bit `*(uint32_t*)`
/// dereferences for unnamed slots, resolves named globals, and preserves tail-call `return imp(...)`
/// on `ldr(.w) pc, [pc, #off]` thunks without swallowing indirect-call GOT loads (`ldr r4, [pc, #off]; blx r4`).
#[test]
fn arm_and_thumb_pc_literal_loads_lift_cleanly_in_pseudo_c() {
    // 1. Mode::A32
    let words: &[u32] = &[
        0xe59f_f000 | 0xff8, // 0x1000: ldr pc, [pc, #0xff8] -> 0x2000
        0xe1a0_0000,         // 0x1004: nop padding
        0xe92d_4010,         // 0x1008: push {r4, lr}
        0xe59f_0ff0,         // 0x100c: ldr r0, [pc, #0xff0] -> 0x2004
        0xe59f_1ff0,         // 0x1010: ldr r1, [pc, #0xff0] -> 0x2008
        0xe59f_4fe4,         // 0x1014: ldr r4, [pc, #0xfe4] -> 0x2000
        0xe12f_ff34,         // 0x1018: blx r4
        0xe8bd_8010,         // 0x101c: pop {r4, pc}
    ];
    let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let slots = vec![ImportSlot { addr: 0x2000, name: "puts".into(), library: Some("libc.so".into()), kind: "got" }];
    let symbols = vec![Symbol {
        name: "g_limit".into(),
        demangled: None,
        addr: 0x2008,
        size: 4,
        kind: SymbolKind::Data,
        global: true,
        weak: false,
        defined: true,
        dynamic: false,
    }];
    let mut img = make_arm_image(0x1008, 8, &code, &[0u8; 16], slots.clone(), symbols);
    img.bin.function_starts.push(0x1000);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze A32 image");

    let caller = prog.functions.get(&0x1008).expect("caller at 0x1008");
    let lifted = pseudo::lift(&img, &prog, caller, &|_| None);
    assert!(lifted.text.contains("*(uint32_t*)0x2004"), "expected 32-bit A32 literal deref:\n{}", lifted.text);
    assert!(!lifted.text.contains("uint64_t"), "32-bit ARM load must not emit uint64_t:\n{}", lifted.text);
    assert!(lifted.text.contains("g_limit"), "expected named global symbol:\n{}", lifted.text);
    assert!(lifted.text.contains("r4 = puts@got;"), "indirect GOT load into r4 must resolve puts@got without being dropped:\n{}", lifted.text);

    let thunk = prog.functions.get(&0x1000).expect("thunk at 0x1000");
    let lifted_thunk = pseudo::lift(&img, &prog, thunk, &|_| None);
    assert!(lifted_thunk.text.contains("return puts("), "A32 GOT indirect jump thunk must not be swallowed:\n{}", lifted_thunk.text);

    // 2. Mode::T32: locks `Mode::T32` 32-bit dereference and Thumb GOT thunk lifting.
    let mut t_code = vec![0u8; 0x18];
    t_code[0x00..0x04].copy_from_slice(&[0xdf, 0xf8, 0xfc, 0xff]); // 0x1000 (.plt): ldr.w pc, [pc, #0xffc] -> 0x2000
    t_code[0x08..0x0c].copy_from_slice(&[0xdf, 0xf8, 0xf8, 0x0f]); // 0x1008 (.text): ldr.w r0, [pc, #0xff8] -> 0x2004
    t_code[0x0c..0x0e].copy_from_slice(&[0x70, 0x47]); // 0x100c: bx lr
    let mut t_img = make_arm_image(0x1009, 8, &t_code, &[0u8; 16], slots, Vec::new());
    t_img.bin.function_starts.push(0x1001);
    let t_prog = Program::analyze(&t_img, &Limits::default(), &NoControl).expect("analyze T32 image");

    let t_leaf = t_prog.functions.get(&0x1008).expect("Thumb leaf at 0x1008");
    let t_lifted = pseudo::lift(&t_img, &t_prog, t_leaf, &|_| None);
    assert!(t_lifted.text.contains("*(uint32_t*)0x2004"), "expected 32-bit Thumb literal deref:\n{}", t_lifted.text);
    assert!(!t_lifted.text.contains("uint64_t"), "Thumb literal load must not emit uint64_t:\n{}", t_lifted.text);

    let t_thunk = t_prog.functions.get(&0x1000).expect("Thumb thunk at 0x1000");
    let t_lifted_thunk = pseudo::lift(&t_img, &t_prog, t_thunk, &|_| None);
    assert!(t_lifted_thunk.text.contains("return puts("), "Thumb GOT thunk must not be swallowed:\n{}", t_lifted_thunk.text);
}

/// A32 allows (deprecated) non-writeback stores with PC as the base: the store keeps its fixed
/// address, without a dynamic `r15` read or a literal reference, while a writeback form stays dynamic.
#[test]
fn a32_pc_relative_stores_keep_their_static_address() {
    for (word, what) in [(0xe58f_0010u32, "str r0, [pc, #0x10]"), (0xe5cf_0010, "strb r0, [pc, #0x10]")] {
        for fmt in [false, true] {
            let insn = decode(Mode::A32, &word.to_le_bytes(), 0x1000, fmt).expect("decode A32 PC store");
            assert!(insn.ops.iter().any(|op| matches!(op, Op::Mem(m) if m.addr == Some(0x1018) && m.base.is_none())), "{what}: {insn:?}");
            assert!(insn.refs.is_empty(), "{what} is not a literal load: {insn:?}");
            assert!(!insn.reads.iter().any(|r| r == "r15"), "{what}: {insn:?}");
        }
    }
    let writeback = decode(Mode::A32, &0xe5af_0010u32.to_le_bytes(), 0x1000, true).expect("decode str r0, [pc, #0x10]!");
    assert!(!writeback.ops.iter().any(|op| matches!(op, Op::Mem(m) if m.addr.is_some())), "{writeback:?}");
    assert!(
        writeback.ops.iter().any(|op| matches!(op, Op::Mem(m) if m.base.as_deref() == Some("r15") && m.addr.is_none())),
        "the writeback form keeps its dynamic r15 operand: {writeback:?}"
    );

    // 0x1008: mov r0, #1; 0x100c: str r0, [pc, #0xff0] -> 0x2004; 0x1010: bx lr
    let words: &[u32] = &[0xe1a0_0000, 0xe1a0_0000, 0xe3a0_0001, 0xe58f_0ff0, 0xe12f_ff1e];
    let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let img = make_arm_image(0x1008, 8, &code, &[0u8; 16], Vec::new(), Vec::new());
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze A32 store");
    let lifted = pseudo::lift(&img, &prog, prog.functions.get(&0x1008).expect("function at 0x1008"), &|_| None);
    assert!(lifted.text.contains("0x2004"), "the store names its fixed address:\n{}", lifted.text);
    assert!(!lifted.text.contains("r15"), "the store must not go through r15:\n{}", lifted.text);
}

/// An `ldr pc` jump through an import slot after setting `r0` is a tail call that passes `r0` and
/// records the import in `Pseudo.calls`.
#[test]
fn a32_import_tail_call_passes_arguments_and_is_recorded() {
    // 0x1008: mov r0, #5; 0x100c: ldr pc, [pc, #0xfec] -> 0x2000 (puts)
    let words: &[u32] = &[0xe1a0_0000, 0xe1a0_0000, 0xe3a0_0005, 0xe59f_ffec];
    let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let slots = vec![ImportSlot { addr: 0x2000, name: "puts".into(), library: Some("libc.so".into()), kind: "got" }];
    let img = make_arm_image(0x1008, 8, &code, &[0u8; 16], slots, Vec::new());
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze A32 tail call");
    let lifted = pseudo::lift(&img, &prog, prog.functions.get(&0x1008).expect("function at 0x1008"), &|_| None);
    assert!(lifted.text.contains("return puts(5); /* tail call */"), "{}", lifted.text);
    assert!(lifted.calls.iter().any(|c| c == "puts"), "calls: {:?}", lifted.calls);
}

/// A tail call's argument load isn't moved past a store that may write the bytes it read: with
/// `str r1, [r4]` or `strb r1, [r4]` between, `puts` is passed `r0` and the load stays; so does a
/// literal load (`ldr r0, [pc, …]`) followed by a store to the same literal. `strb` isn't modelled,
/// so its width is unknown and even a load from `[r4, #8]` stays. A `str` to `[r4, #4]` doesn't
/// overlap `[r4]`, so the load folds into the call and its own statement goes.
#[test]
fn a32_argument_load_is_not_moved_past_an_overlapping_store() {
    // 0x1008: <load>; 0x100c: <store>; 0x1010: ldr pc, [pc, #0xfe8] -> 0x2000 (puts)
    let (ldr_r4, ldr_lit) = (0xe594_0000u32, 0xe59f_0ff4); // ldr r0, [r4]; ldr r0, [pc, #0xff4] -> 0x2004
    for (load, store, folded) in [
        (ldr_r4, 0xe584_1000u32, false),   // str r1, [r4]
        (ldr_r4, 0xe5c4_1000, false),      // strb r1, [r4]
        (0xe594_0008, 0xe5c4_1000, false), // ldr r0, [r4, #8]; strb r1, [r4]
        (ldr_lit, 0xe58f_1ff0, false),     // str r1, [pc, #0xff0] -> 0x2004
        (ldr_r4, 0xe584_1004, true),       // str r1, [r4, #4]
    ] {
        let words: &[u32] = &[0xe1a0_0000, 0xe1a0_0000, load, store, 0xe59f_ffe8];
        let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let slots = vec![ImportSlot { addr: 0x2000, name: "puts".into(), library: Some("libc.so".into()), kind: "got" }];
        let img = make_arm_image(0x1008, 8, &code, &[0u8; 16], slots, Vec::new());
        let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze A32 tail call");
        let lifted = pseudo::lift(&img, &prog, prog.functions.get(&0x1008).expect("function at 0x1008"), &|_| None);
        let call = lifted.text.lines().find(|l| l.contains("/* tail call */")).unwrap_or_default().trim().to_string();
        let load_stays = lifted.text.lines().any(|l| l.trim_start().starts_with("r0 = "));
        if folded {
            assert_eq!(call, "return puts(*(uint32_t*)(r4)); /* tail call */", "{}", lifted.text);
            assert!(!load_stays, "the folded load must not stay as well:\n{}", lifted.text);
        } else {
            assert_eq!(call, "return puts(r0); /* tail call */", "{store:#x}:\n{}", lifted.text);
            assert!(load_stays, "the load must stay:\n{}", lifted.text);
        }
    }
}

/// A store's source register isn't its destination: `str r1, [r4]` before a tail call keeps its
/// statement instead of being folded away as `r1`'s value.
#[test]
fn a32_store_before_a_call_keeps_its_statement() {
    // 0x1008: mov r1, #2; 0x100c: str r1, [r4]; 0x1010: ldr pc, [pc, #0xfe8] -> 0x2000 (puts)
    let words: &[u32] = &[0xe1a0_0000, 0xe1a0_0000, 0xe3a0_1002, 0xe584_1000, 0xe59f_ffe8];
    let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let slots = vec![ImportSlot { addr: 0x2000, name: "puts".into(), library: Some("libc.so".into()), kind: "got" }];
    let img = make_arm_image(0x1008, 8, &code, &[0u8; 16], slots, Vec::new());
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze A32 store and tail call");
    let lifted = pseudo::lift(&img, &prog, prog.functions.get(&0x1008).expect("function at 0x1008"), &|_| None);
    assert!(lifted.text.contains("*(uint32_t*)(r4) = r1;"), "the store must stay:\n{}", lifted.text);
    assert!(lifted.text.contains("return puts(r0); /* tail call */"), "{}", lifted.text);
}

/// `STRB`, `STRH` and `STRD` read the registers they store, like `STR`; none of them writes one. So
/// a function that only stores `r0` still takes `r0` as a parameter.
#[test]
fn a32_narrow_and_double_stores_read_their_source_registers() {
    for (word, what, sources) in
        [(0xe5c4_1000u32, "strb r1, [r4]", &["r1"][..]), (0xe1c4_10b0, "strh r1, [r4]", &["r1"][..]), (0xe1c4_20f0, "strd r2, r3, [r4]", &["r2", "r3"][..])]
    {
        let insn = decode(Mode::A32, &word.to_le_bytes(), 0x1000, true).expect(what);
        for r in sources {
            assert!(insn.reads.iter().any(|x| x == r), "{what} reads {r}: {insn:?}");
            assert!(!insn.writes.iter().any(|x| x == r), "{what} doesn't write {r}: {insn:?}");
        }
    }
    // 0x1008: strb r0, [r4]; 0x100c: bx lr
    let words: &[u32] = &[0xe1a0_0000, 0xe1a0_0000, 0xe5c4_0000, 0xe12f_ff1e];
    let code: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let img = make_arm_image(0x1008, 8, &code, &[0u8; 16], Vec::new(), Vec::new());
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze A32 strb");
    let lifted = pseudo::lift(&img, &prog, prog.functions.get(&0x1008).expect("function at 0x1008"), &|_| None);
    let signature = lifted.text.lines().next().unwrap_or_default();
    assert!(signature.contains("r0"), "r0 is read before any write, so it is a parameter:\n{}", lifted.text);
}
