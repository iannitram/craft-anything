//! Regressions for eBPF / sBPF `ja32` (`0x06`, RFC 9669 §4.3) unconditional 32-bit slot jumps,
//! rejection of undefined `BPF_X` jump/exit encodings (`0x0d`, `0x0e`, and `V1` `0x9d`), and
//! preservation of Solana `V2` `return` (`0x9d`).

use craft_anything_disasm::sbpf::{Flow, SbpfVersion, analyze, decode, decode_one, pseudo};

/// Encode one 8-byte little-endian eBPF/sBPF instruction slot.
fn slot(opcode: u8, dst: u8, src: u8, off: i16, imm: i32) -> [u8; 8] {
    let mut raw = [0u8; 8];
    raw[0] = opcode;
    raw[1] = (src << 4) | (dst & 0x0f);
    raw[2..4].copy_from_slice(&off.to_le_bytes());
    raw[4..8].copy_from_slice(&imm.to_le_bytes());
    raw
}

/// `0x06` (`BPF_JMP32 | BPF_K | BPF_JA`) is an unconditional `ja32` whose PC-relative slot offset
/// comes from `imm32` (ignoring `off`), producing `Flow::Jump` with no fallthrough edge.
#[test]
fn ja32_decodes_unconditional_32bit_slot_jump_and_cfg() {
    // Forward jump past 16-bit range (`imm = 0x1_0000` slots), with non-zero `off` distraction.
    let far = slot(0x06, 2, 3, 99, 0x1_0000);
    let d_far = decode_one(&far, 0, 0x1000, SbpfVersion::V1).expect("decode ja32");
    let expected_far = 0x1000 + (1u64 + 0x1_0000) * 8;
    assert_eq!(d_far.mnemonic, "ja32");
    assert_eq!(d_far.operands, format!("{expected_far:#x}"));
    assert_eq!(d_far.flow, Flow::Jump { target: expected_far });

    // Backward jump (`imm = -2` from slot 5 lands on slot 4 = `0x1020`).
    let mut padded = vec![0u8; 6 * 8];
    padded[5 * 8..6 * 8].copy_from_slice(&slot(0x06, 0, 0, 7, -2));
    let d_back = decode_one(&padded, 5 * 8, 0x1000, SbpfVersion::V2).expect("decode backward ja32");
    assert_eq!(d_back.mnemonic, "ja32");
    assert_eq!(d_back.operands, "0x1020");
    assert_eq!(d_back.flow, Flow::Jump { target: 0x1020 });

    // End-to-end CFG and pseudo listing: `ja32 +2` at slot 0 skips unreachable slots 1..=2.
    let mut code = Vec::new();
    code.extend_from_slice(&slot(0x06, 0, 0, 0, 2)); // 0x100 (slot 0): ja32 +2 -> 0x118 (slot 3)
    code.extend_from_slice(&slot(0xb7, 0, 0, 0, 99)); // 0x108 (slot 1): unreachable mov64 r0, 99
    code.extend_from_slice(&slot(0x95, 0, 0, 0, 0)); // 0x110 (slot 2): unreachable exit
    code.extend_from_slice(&slot(0xb7, 0, 0, 0, 7)); // 0x118 (slot 3): mov64 r0, 7
    code.extend_from_slice(&slot(0x95, 0, 0, 0, 0)); // 0x120 (slot 4): exit

    let insns = decode(&code, 0x100, SbpfVersion::V1);
    let analysis = analyze(&insns, &[(0x100, Some("entry".to_string()))]);
    assert_eq!(analysis.functions.len(), 1);
    let func = &analysis.functions[0];
    assert_eq!(func.block_count, 2, "{func:?}");
    assert_eq!(func.instruction_count, 3, "{func:?}");

    let entry_block = func.blocks.iter().find(|b| b.start == 0x100).expect("entry block");
    assert_eq!(entry_block.terminator, "jump");
    assert_eq!(entry_block.successors, vec![0x118]);

    let listing = pseudo(func, &insns, &|_| None);
    assert!(listing.contains("    goto L_118;\n"), "{listing}");
    assert!(!listing.contains("if ("), "{listing}");
    assert!(!listing.contains("0x63"), "unreachable slot leaked into function:\n{listing}");
}

/// Undefined `BPF_X` (`src` bit set) variants of `JA` (`0x0d`, `0x0e`) and `V1` `EXIT` (`0x9d`)
/// decode as `Flow::Unknown` terminators, while Solana `V2` keeps `0x9d` as `return` (`Flow::Exit`).
#[test]
fn undefined_bpf_x_ja_and_v1_exit_are_unknown_while_v2_return_stays_exit() {
    for version in [SbpfVersion::V1, SbpfVersion::V2] {
        for opcode in [0x0du8, 0x0e] {
            let raw = slot(opcode, 1, 2, 1, 1);
            let insn = decode_one(&raw, 0, 0x200, version).expect("decode 8-byte slot");
            assert_eq!(insn.mnemonic, format!("(jmp?{opcode:#04x})"), "{version:?} opcode {opcode:#04x}");
            assert_eq!(insn.flow, Flow::Unknown, "{version:?} opcode {opcode:#04x}");

            let mut code = Vec::new();
            code.extend_from_slice(&slot(opcode, 0, 0, 1, 0));
            code.extend_from_slice(&slot(0xb7, 0, 0, 0, 1));
            code.extend_from_slice(&slot(0x95, 0, 0, 0, 0));
            let insns = decode(&code, 0, version);
            let analysis = analyze(&insns, &[(0, Some("entry".to_string()))]);
            let f = &analysis.functions[0];
            assert_eq!(f.blocks.len(), 1);
            assert_eq!(f.blocks[0].terminator, "unknown");
            assert!(f.blocks[0].successors.is_empty());
            let listing = pseudo(f, &insns, &|_| None);
            assert!(!listing.contains("goto "), "{listing}");
            assert!(!listing.contains("if ("), "{listing}");
        }
    }

    // `0x9d` in V1 (classic eBPF / sBPF v1) is undefined, not `exit`.
    let v1_9d = decode_one(&slot(0x9d, 0, 0, 0, 0), 0, 0, SbpfVersion::V1).expect("decode V1 0x9d");
    assert_eq!(v1_9d.mnemonic, "(jmp?0x9d)");
    assert_eq!(v1_9d.flow, Flow::Unknown);
    let mut v1_code = Vec::new();
    v1_code.extend_from_slice(&slot(0x9d, 0, 0, 0, 0));
    v1_code.extend_from_slice(&slot(0xb7, 0, 0, 0, 1));
    let v1_insns = decode(&v1_code, 0, SbpfVersion::V1);
    let v1_analysis = analyze(&v1_insns, &[(0, Some("entry".to_string()))]);
    assert_eq!(v1_analysis.functions[0].blocks[0].terminator, "unknown");
    assert!(!pseudo(&v1_analysis.functions[0], &v1_insns, &|_| None).contains("return;"));

    // Control: `0x9d` in V2+ (Solana SIMD-0178 `return`) continues to decode as `return` / `Flow::Exit`.
    let v2_9d = decode_one(&slot(0x9d, 0, 0, 0, 0), 0, 0, SbpfVersion::V2).expect("decode V2 0x9d");
    assert_eq!(v2_9d.mnemonic, "return");
    assert!(v2_9d.operands.is_empty());
    assert_eq!(v2_9d.flow, Flow::Exit);
    let v2_insns = decode(&v1_code, 0, SbpfVersion::V2);
    let v2_analysis = analyze(&v2_insns, &[(0, Some("entry".to_string()))]);
    assert_eq!(v2_analysis.functions[0].blocks[0].terminator, "exit");
    assert!(pseudo(&v2_analysis.functions[0], &v2_insns, &|_| None).contains("    return;\n"));
}

/// Full 64-opcode truth table across `CLS_JMP` (`0x05`) and `CLS_JMP32` (`0x06`) keeps classic
/// `ja`, `call`, `callx`, `exit`/`return` (including `V2` `0x9d`), and all 44 conditional jump
/// opcodes intact while flagging all remaining reserved opcodes as `Flow::Unknown`.
#[test]
fn full_jmp_and_jmp32_opcode_matrix_matches_spec() {
    for version in [SbpfVersion::V1, SbpfVersion::V2] {
        for class in [0x05u8, 0x06u8] {
            for src_bit in [0x00u8, 0x08u8] {
                for hi in 0u8..16 {
                    let nib = hi << 4;
                    let opcode = nib | src_bit | class;
                    let raw = slot(opcode, 1, 2, 3, 5);
                    let insn = decode_one(&raw, 0, 0x1000, version).expect("decode slot");

                    match (nib, src_bit != 0, class) {
                        // Unconditional 16-bit offset jump: target = 0x1000 + (1 + 3) * 8 = 0x1020.
                        (0x00, false, 0x05) => {
                            assert_eq!(insn.mnemonic, "ja");
                            assert_eq!(insn.operands, "0x1020");
                            assert_eq!(insn.flow, Flow::Jump { target: 0x1020 });
                        }
                        // Unconditional 32-bit immediate jump: target = 0x1000 + (1 + 5) * 8 = 0x1030.
                        (0x00, false, 0x06) => {
                            assert_eq!(insn.mnemonic, "ja32");
                            assert_eq!(insn.operands, "0x1030");
                            assert_eq!(insn.flow, Flow::Jump { target: 0x1030 });
                        }
                        // Conditional 64-bit and 32-bit branches (11 operations * 4 = 44 opcodes).
                        (0x10..=0x70 | 0xa0..=0xd0, use_src, cls) => {
                            assert_eq!(insn.flow, Flow::Branch { target: 0x1020 }, "opcode {opcode:#04x}");
                            assert_eq!(insn.mnemonic.ends_with("32"), cls == 0x06, "opcode {opcode:#04x}");
                            let expected_rhs = if use_src { "r2" } else { "0x5" };
                            assert_eq!(insn.operands, format!("r1, {expected_rhs}, 0x1020"), "opcode {opcode:#04x}");
                        }
                        // `call imm` (`0x85`).
                        (0x80, false, 0x05) => {
                            assert_eq!(insn.mnemonic, "call");
                            match version {
                                SbpfVersion::V1 => assert_eq!(insn.flow, Flow::CallHash { imm: 5 }),
                                SbpfVersion::V2 => assert_eq!(insn.flow, Flow::Call { target: 0x1030 }),
                            }
                        }
                        // `callx` (`0x8d`).
                        (0x80, true, 0x05) => {
                            assert_eq!(insn.mnemonic, "callx");
                            assert_eq!(insn.flow, Flow::CallReg);
                        }
                        // `exit` (`0x95` in V1) / `return` (`0x95` and `0x9d` in V2).
                        (0x90, use_src, 0x05) if !use_src || version == SbpfVersion::V2 => {
                            let expected = if version == SbpfVersion::V2 { "return" } else { "exit" };
                            assert_eq!(insn.mnemonic, expected);
                            assert_eq!(insn.flow, Flow::Exit);
                        }
                        // Reserved opcodes across CLS_JMP / CLS_JMP32 (15 in V1, 14 in V2).
                        _ => {
                            assert_eq!(insn.mnemonic, format!("(jmp?{opcode:#04x})"), "opcode {opcode:#04x}");
                            assert_eq!(insn.flow, Flow::Unknown, "opcode {opcode:#04x}");
                        }
                    }
                }
            }
        }
    }
}
