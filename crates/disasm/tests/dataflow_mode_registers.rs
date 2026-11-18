//! Regressions for architecture-scoped register filtering (`r13`/`r15` on ARM32/Thumb vs
//! general-purpose `r13`–`r15` on x86-64) and canonical scratch clobber names on 32-/16-bit x86
//! in `craft_anything_disasm::dataflow::def_use`.

use craft_anything_disasm::dataflow::def_use;
use craft_anything_disasm::{Image, Limits, NoControl, Program};
use craft_anything_formats::{Arch, Binary};
use std::sync::Arc;

/// Build and analyze a raw single-function image at `0x1000` (with optional Thumb entry bit).
fn raw_def_use(bytes: Vec<u8>, arch: Arch, thumb: bool) -> craft_anything_disasm::dataflow::DefUse {
    let mut bin = Binary::raw(arch, 0x1000, bytes.len() as u64, 0);
    if thumb {
        bin.entry = Some(0x1001);
    }
    let img = Image::from_binary(Arc::from(bytes), bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).expect("analyze raw function");
    let func = prog.functions.get(&0x1000).expect("function at 0x1000");
    let flow = def_use(&img, &prog, func, 500, 1000);
    assert!(!flow.truncated, "{flow:?}");
    flow
}

/// ARM32 and Thumb `def_use` must ignore stack-pointer (`r13`) and program-counter (`r15`)
/// bookkeeping on `defines`, `edges`, and store `Effect.inputs` (matching `rsp`/`sp`/`rip` on
/// x86-64 and AArch64) while keeping real data registers like `r0` and `r4`.
#[test]
fn arm_and_thumb_def_use_ignores_sp_and_pc_bookkeeping() {
    // A32: push {r4, lr}; str r0, [sp]; add r0, r0, #1; pop {r4, pc}
    let a32: Vec<u8> = [0xe92d_4010u32, 0xe58d_0000, 0xe280_0001, 0xe8bd_8010].into_iter().flat_map(u32::to_le_bytes).collect();
    // Thumb: push {r4, lr}; str r0, [sp, #0]; adds r0, #1; pop {r4, pc}
    let thumb: Vec<u8> = vec![0x10, 0xb5, 0x00, 0x90, 0x01, 0x30, 0x10, 0xbd];

    for (label, bytes, is_thumb, pop_addr) in [("A32", a32, false, 0x100cu64), ("T32", thumb, true, 0x1006u64)] {
        let flow = raw_def_use(bytes, Arch::Arm, is_thumb);
        for node in &flow.nodes {
            assert!(
                !node.defines.iter().any(|r| matches!(r.as_str(), "r13" | "r15")),
                "{label}: node {:?} leaked SP/PC into defines: {:?}",
                node.addr,
                node.defines
            );
        }
        for edge in &flow.edges {
            assert!(!matches!(edge.register.as_str(), "r13" | "r15"), "{label}: unexpected SP/PC edge: {edge:?}");
        }
        let push_node = flow.nodes.iter().find(|n| n.addr == Some(0x1000)).expect("push node");
        assert!(push_node.defines.is_empty(), "{label}: push should define no tracked GPRs: {push_node:?}");

        let pop_node = flow.nodes.iter().find(|n| n.addr == Some(pop_addr)).expect("pop node");
        assert_eq!(pop_node.defines, vec!["r4".to_string()], "{label}: pop {pop_node:?}");

        let store = flow.effects.iter().find(|e| e.kind == "store").expect("stack store effect");
        assert_eq!(store.inputs, vec!["r0".to_string()], "{label}: store inputs should exclude r13: {store:?}");
    }

    // Non-terminal `pc` write + subsequent `pc` reads (`popeq {r4, pc}; add r0, pc, #8;
    // str pc, [r1]; pop {r4, pc}`) verify `r15` is filtered from `DfEdge` and store `Effect.inputs`
    // as well as `DfNode.defines`.
    let pc_reads: Vec<u8> = [0x08bd_8010u32, 0xe28f_0008, 0xe581_f000, 0xe8bd_8010].into_iter().flat_map(u32::to_le_bytes).collect();
    let pc_flow = raw_def_use(pc_reads, Arch::Arm, false);
    assert!(!pc_flow.nodes.iter().any(|n| n.defines.iter().any(|r| matches!(r.as_str(), "r13" | "r15"))), "{pc_flow:?}");
    assert!(!pc_flow.edges.iter().any(|e| matches!(e.register.as_str(), "r13" | "r15")), "{pc_flow:?}");
    let pc_store = pc_flow.effects.iter().find(|e| e.kind == "store").expect("str pc, [r1] effect");
    assert_eq!(pc_store.inputs, vec!["r1".to_string()], "store inputs should exclude r15: {pc_store:?}");

    // Once saved on the stack, `lr` (`r14`) is frequently reused as a general-purpose scratch
    // register on A32 (`push {r4, lr}; mov lr, #9; add r0, r0, lr; pop {r4, pc}`): keep its edge.
    let lr_scratch: Vec<u8> = [0xe92d_4010u32, 0xe3a0_e009, 0xe080_000e, 0xe8bd_8010].into_iter().flat_map(u32::to_le_bytes).collect();
    let lr_flow = raw_def_use(lr_scratch, Arch::Arm, false);
    let set_lr = lr_flow.nodes.iter().find(|n| n.addr == Some(0x1004)).expect("mov lr, #9");
    let use_lr = lr_flow.nodes.iter().find(|n| n.addr == Some(0x1008)).expect("add r0, r0, lr");
    assert_eq!(set_lr.defines, vec!["r14".to_string()]);
    assert!(lr_flow.edges.iter().any(|e| e.from == set_lr.id && e.to == use_lr.id && e.register == "r14"), "{lr_flow:?}");
}

/// On `Mode::X64`, `r13` and `r15` are general-purpose registers (not SP/PC) and must retain
/// their `defines` and def-use edges while `rsp` remains ignored.
#[test]
fn x86_64_general_purpose_r13_and_r15_keep_their_def_use_edges() {
    // push r13 (41 55); mov r13d, 7 (41 bd 07 00 00 00); mov r15d, 3 (41 bf 03 00 00 00);
    // mov eax, r13d (44 89 e8); add eax, r15d (44 01 f8); pop r13 (41 5d); ret (c3)
    let code = vec![0x41, 0x55, 0x41, 0xbd, 7, 0, 0, 0, 0x41, 0xbf, 3, 0, 0, 0, 0x44, 0x89, 0xe8, 0x44, 0x01, 0xf8, 0x41, 0x5d, 0xc3];
    let flow = raw_def_use(code, Arch::X86_64, false);

    let set_r13 = flow.nodes.iter().find(|n| n.addr == Some(0x1002)).expect("mov r13d, 7");
    let set_r15 = flow.nodes.iter().find(|n| n.addr == Some(0x1008)).expect("mov r15d, 3");
    let mov_eax = flow.nodes.iter().find(|n| n.addr == Some(0x100e)).expect("mov eax, r13d");
    let add_eax = flow.nodes.iter().find(|n| n.addr == Some(0x1011)).expect("add eax, r15d");

    assert_eq!(set_r13.defines, vec!["r13".to_string()]);
    assert_eq!(set_r15.defines, vec!["r15".to_string()]);
    assert!(flow.edges.iter().any(|e| e.from == set_r13.id && e.to == mov_eax.id && e.register == "r13"), "{flow:?}");
    assert!(flow.edges.iter().any(|e| e.from == set_r15.id && e.to == add_eax.id && e.register == "r15"), "{flow:?}");
    assert!(!flow.edges.iter().any(|e| e.register == "rsp"), "{flow:?}");
}

/// 32-bit and 16-bit x86 decoders normalize register names to `"rax"`, `"rcx"`, `"rdx"`, so
/// `call` nodes must clobber only those canonical names instead of emitting duplicate dead
/// `"eax"`, `"ecx"`, `"edx"` entries.
#[test]
fn x86_32_and_x86_16_calls_clobber_only_canonical_registers() {
    // 32-bit x86 at 0x1000:
    //   0x1000: mov ecx, 5       (b9 05 00 00 00)
    //   0x1005: call 0x1010      (e8 06 00 00 00)
    //   0x100a: add eax, ecx     (01 c8)
    //   0x100c: ret              (c3)
    //   0x1010 <callee>: ret     (c3)
    let mut x32 = vec![0x90u8; 0x14];
    x32[0x00..0x0d].copy_from_slice(&[0xb9, 5, 0, 0, 0, 0xe8, 6, 0, 0, 0, 0x01, 0xc8, 0xc3]);
    x32[0x10] = 0xc3;
    let flow32 = raw_def_use(x32, Arch::X86, false);

    let mov_ecx = flow32.nodes.iter().find(|n| n.addr == Some(0x1000)).expect("mov ecx, 5");
    let call32 = flow32.nodes.iter().find(|n| n.addr == Some(0x1005)).expect("call 0x1010");
    let add32 = flow32.nodes.iter().find(|n| n.addr == Some(0x100a)).expect("add eax, ecx");
    let ret32 = flow32.nodes.iter().find(|n| n.addr == Some(0x100c)).expect("ret");

    assert_eq!(call32.defines, vec!["rax".to_string(), "rcx".to_string(), "rdx".to_string()], "{call32:?}");
    assert!(!flow32.edges.iter().any(|e| e.from == mov_ecx.id && e.to == add32.id && e.register == "rcx"), "call must kill earlier ecx/rcx: {flow32:?}");
    assert!(flow32.edges.iter().any(|e| e.from == call32.id && e.to == add32.id && e.register == "rcx"), "{flow32:?}");
    assert!(flow32.edges.iter().any(|e| e.from == add32.id && e.to == ret32.id && e.register == "rax"), "{flow32:?}");

    // 16-bit x86 at 0x1000: call 0x1006 (e8 03 00); ret (c3); pad; ret (c3)
    let x16 = vec![0xe8, 3, 0, 0xc3, 0x90, 0x90, 0xc3];
    let flow16 = raw_def_use(x16, Arch::X86_16, false);
    let call16 = flow16.nodes.iter().find(|n| n.addr == Some(0x1000)).expect("16-bit call");
    assert_eq!(call16.defines, vec!["rax".to_string(), "rcx".to_string(), "rdx".to_string()], "{call16:?}");
}
