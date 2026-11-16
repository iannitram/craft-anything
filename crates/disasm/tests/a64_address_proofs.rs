use craft_anything_disasm::{ArmConsts, Flow, Image, Insn, Limits, Mode, NoControl, Program, decode};
use craft_anything_testkit as tk;

fn apply(constants: &mut ArmConsts, word: u32, address: u64) -> Insn {
    let mut instruction = decode(Mode::A64, &word.to_le_bytes(), address, true).unwrap();
    constants.apply(&mut instruction);
    instruction
}

fn known_base(address: u64) -> ArmConsts {
    let mut constants = ArmConsts::default();
    apply(&mut constants, 0x9000_0001, address); // adrp x1, current page
    constants
}

fn elf_program(tail: &[u32]) -> (Program, u64, u64) {
    let (mut bytes, layout) = tk::elf64(tk::ElfArch::Aarch64, false);
    let pages = (layout.got_va >> 12).wrapping_sub(layout.code_va >> 12) & 0x1f_ffff;
    let adrp = 0x9000_0001 | (((pages & 3) as u32) << 29) | (((pages >> 2) as u32) << 5);
    let add = 0x9100_0021 | (((layout.got_va & 0xfff) as u32) << 10);
    let words = [adrp, add].into_iter().chain(tail.iter().copied());
    let code = words.flat_map(u32::to_le_bytes).collect::<Vec<_>>();
    let start = tk::ELF_TEXT_OFF as usize;
    bytes[start..start + tk::a64::CODE_SIZE].fill(0);
    bytes[start..start + code.len()].copy_from_slice(&code);
    let image = Image::parse(bytes.into()).unwrap();
    let program = Program::analyze(&image, &Limits { scan_prologues: false, ..Limits::default() }, &NoControl).unwrap();
    (program, layout.code_va, layout.got_va)
}

#[test]
fn a_runtime_addend_does_not_become_a_zero_offset() {
    for word in [0x8b02_0020, 0xab02_0020, 0x0b02_0020, 0x8b02_0c20, 0x8b22_4020] {
        let mut constants = known_base(0x1000);
        apply(&mut constants, 0x9000_0000, 0x1000); // stale destination value must also go
        let instruction = apply(&mut constants, word, 0x1004);
        assert!(instruction.refs.is_empty(), "{}: {:?}", instruction.text(), instruction.refs);
        assert_eq!(constants.value("x0"), None, "{}", instruction.text());
    }
}

#[test]
fn runtime_addends_do_not_name_an_imported_function() {
    let (program, entry, slot) = elf_program(&[0x8b02_0020, 0xf940_0010, 0xd63f_0200, 0xd65f_03c0]);
    let function = program.functions.get(&entry).unwrap();
    assert_eq!(function.calls.len(), 1);
    assert!(function.calls[0].import.is_none(), "{:?}", function.calls);
    assert!(!program.xrefs_from.get(&(entry + 12)).is_some_and(|refs| refs.iter().any(|reference| reference.to == slot)));
}

#[test]
fn word_additions_zero_extend_the_wrapped_result() {
    let mut constants = known_base(0x1_0000_0000);
    let add = apply(&mut constants, 0x1100_0420, 0x1_0000_0004); // add w0, w1, #1
    assert_eq!(constants.value("x0"), Some(1));
    assert_eq!(add.refs, vec![1]);

    let mut constants = known_base(0x1_ffff_f000);
    apply(&mut constants, 0x113f_fc21, 0x1_ffff_f004); // add w1, w1, #4095
    let add = apply(&mut constants, 0x1100_0420, 0x1_ffff_f008);
    assert_eq!(constants.value("x0"), Some(0));
    assert_eq!(add.refs, vec![0]);
}

#[test]
fn runtime_memory_indices_do_not_prove_a_memory_address() {
    for word in [0xf862_6830, 0xf822_6820] {
        // ldr x16, [x1,x2]; str x0, [x1,x2]
        let mut constants = known_base(0x1000);
        let instruction = apply(&mut constants, word, 0x1004);
        assert!(instruction.refs.is_empty(), "{}: {:?}", instruction.text(), instruction.refs);
        if word == 0xf862_6830 {
            let branch = apply(&mut constants, 0xd61f_0200, 0x1008);
            assert_eq!(branch.flow, Flow::IndirectJump { slot: None });
        }
        assert_eq!(constants.value("x1"), Some(0x1000));
    }
}

#[test]
fn indexed_loads_do_not_name_an_imported_function() {
    let (program, entry, slot) = elf_program(&[0xf862_6830, 0xd63f_0200, 0xd65f_03c0]);
    let function = program.functions.get(&entry).unwrap();
    assert_eq!(function.calls.len(), 1);
    assert!(function.calls[0].import.is_none(), "{:?}", function.calls);
    assert!(!program.xrefs_from.get(&(entry + 8)).is_some_and(|refs| refs.iter().any(|reference| reference.to == slot)));
}

#[test]
fn zero_register_indices_still_resolve_their_load_slot() {
    // `ldr x16, [x1, xzr]` offsets by zero: the zero register is no index, so the known base
    // still proves the slot and the branch through the loaded pointer keeps it.
    let mut constants = known_base(0x1000);
    let load = apply(&mut constants, 0xf87f_6830, 0x1004);
    assert_eq!(load.refs, vec![0x1000], "{}", load.text());
    let branch = apply(&mut constants, 0xd61f_0200, 0x1008); // br x16
    assert_eq!(branch.flow, Flow::IndirectJump { slot: Some(0x1000) });
}

#[test]
fn writeback_loads_retire_the_old_base_and_keep_the_loaded_slot() {
    for (word, source) in [(0xf840_8420, 0x1000), (0xf840_8c20, 0x1008)] {
        let mut constants = known_base(0x1000);
        let load = apply(&mut constants, word, 0x1004);
        assert_eq!(load.refs, vec![source]);
        assert_eq!(constants.value("x1"), None, "{} kept its old base", load.text());
        let branch = apply(&mut constants, 0xd61f_0000, 0x1008); // br x0
        assert_eq!(branch.flow, Flow::IndirectJump { slot: Some(source) });
    }
}

#[test]
fn writeback_bases_do_not_name_the_previous_import_slot() {
    for word in [0xf840_8420, 0xf840_8c20] {
        let (program, entry, slot) = elf_program(&[word, 0xf940_0030, 0xd63f_0200, 0xd65f_03c0]);
        let function = program.functions.get(&entry).unwrap();
        assert_eq!(function.calls.len(), 1);
        assert!(function.calls[0].import.is_none(), "{:?}", function.calls);
        assert!(!program.xrefs_from.get(&(entry + 12)).is_some_and(|refs| refs.iter().any(|reference| reference.to == slot)));
    }
}

#[test]
fn calls_retire_a_link_registers_old_pointer_slot() {
    let mut constants = known_base(0x1000);
    apply(&mut constants, 0xf940_003e, 0x1004); // ldr x30, [x1]
    apply(&mut constants, 0xf940_0033, 0x1008); // ldr x19, [x1]
    apply(&mut constants, 0xd63f_0040, 0x100c); // blr x2
    let link_branch = apply(&mut constants, 0xd61f_03c0, 0x1010); // br x30
    assert_eq!(link_branch.flow, Flow::IndirectJump { slot: None });
    let saved_branch = apply(&mut constants, 0xd61f_0260, 0x1014); // br x19
    assert_eq!(saved_branch.flow, Flow::IndirectJump { slot: Some(0x1000) });
}

#[test]
fn known_immediates_and_ordinary_import_loads_stay_resolved() {
    let mut constants = known_base(0x1_0000_0000);
    let add = apply(&mut constants, 0x9100_6020, 0x1_0000_0004); // add x0, x1, #24
    assert_eq!(constants.value("x0"), Some(0x1_0000_0018));
    assert_eq!(add.refs, vec![0x1_0000_0018]);
    let shifted = apply(&mut constants, 0x9140_0420, 0x1_0000_0008); // add x0, x1, #1, lsl #12
    assert_eq!(shifted.refs, vec![0x1_0000_1000]);
    let zero = apply(&mut constants, 0x8b1f_0020, 0x1_0000_000c); // add x0, x1, xzr
    assert_eq!(zero.refs, vec![0x1_0000_0000]);

    let (program, entry, slot) = elf_program(&[0x9100_0020, 0xf940_0010, 0xd63f_0200, 0xd65f_03c0]);
    assert_eq!(program.functions.get(&entry).unwrap().calls[0].import.as_deref(), Some("puts"));
    assert!(program.xrefs_from.get(&(entry + 12)).unwrap().iter().any(|reference| reference.to == slot));
}
