use craft_anything_disasm::{Image, Insn, Limits, Mode, NoControl, Program, dataflow, decode};
use craft_anything_formats::{Arch, Binary};
use std::sync::Arc;

fn registers(i: &Insn, reads: &[&str], writes: &[&str]) {
    let mut actual_reads = i.reads.iter().map(String::as_str).collect::<Vec<_>>();
    let mut actual_writes = i.writes.iter().map(String::as_str).collect::<Vec<_>>();
    let mut expected_reads = reads.to_vec();
    let mut expected_writes = writes.to_vec();
    actual_reads.sort_unstable();
    actual_writes.sort_unstable();
    expected_reads.sort_unstable();
    expected_writes.sort_unstable();
    assert_eq!(actual_reads, expected_reads, "{} reads", i.text());
    assert_eq!(actual_writes, expected_writes, "{} writes", i.text());
}

/// Multi-register `pop` in Thumb and A32 writes every popped register without marking any popped
/// register as read.
#[test]
fn a32_and_thumb_pop_writes_all_registers_in_list() {
    // Thumb `pop {r0, r1}` (0xbc03)
    let thumb_pop = decode(Mode::T32, &[0x03, 0xbc], 0x1000, true).unwrap();
    registers(&thumb_pop, &["r13"], &["r0", "r1", "r13"]);

    // A32 `pop {r0, r1}` / `ldmia sp!, {r0, r1}` (0xe8bd_0003)
    let a32_pop = decode(Mode::A32, &0xe8bd_0003u32.to_le_bytes(), 0x1000, true).unwrap();
    registers(&a32_pop, &["r13"], &["r0", "r1", "r13"]);

    // Thumb `pop {r4, pc}` (0xbd10) and A32 `pop {r4, pc}` (0xe8bd_8010)
    let thumb_ret = decode(Mode::T32, &[0x10, 0xbd], 0x1000, true).unwrap();
    registers(&thumb_ret, &["r13"], &["r4", "r13", "r15"]);
    let a32_ret = decode(Mode::A32, &0xe8bd_8010u32.to_le_bytes(), 0x1000, true).unwrap();
    registers(&a32_ret, &["r13"], &["r4", "r13", "r15"]);
}

/// `ldm` reads its base pointer (and writes it back only with `!`) while writing all loaded
/// registers; `stm` reads its base pointer and all stored registers.
#[test]
fn a32_ldm_and_stm_keep_base_and_register_list_directions() {
    // ldm r2, {r0, r1}
    registers(&decode(Mode::A32, &0xe892_0003u32.to_le_bytes(), 0x1000, true).unwrap(), &["r2"], &["r0", "r1"]);
    // ldm r2!, {r0, r1}
    registers(&decode(Mode::A32, &0xe8b2_0003u32.to_le_bytes(), 0x1000, true).unwrap(), &["r2"], &["r0", "r1", "r2"]);
    // stm r2, {r0, r1}
    registers(&decode(Mode::A32, &0xe882_0003u32.to_le_bytes(), 0x1000, true).unwrap(), &["r0", "r1", "r2"], &[]);
    // stm r2!, {r0, r1}
    registers(&decode(Mode::A32, &0xe8a2_0003u32.to_le_bytes(), 0x1000, true).unwrap(), &["r0", "r1", "r2"], &["r2"]);
}

/// `push` reads every register in its list plus `r13` and writes `r13` writeback in both Thumb
/// and A32.
#[test]
fn a32_and_thumb_push_record_listed_reads_and_sp_writeback() {
    registers(&decode(Mode::T32, &[0x10, 0xb5], 0x1000, true).unwrap(), &["r4", "r13", "r14"], &["r13"]);
    registers(&decode(Mode::A32, &0xe92d_4010u32.to_le_bytes(), 0x1000, true).unwrap(), &["r4", "r13", "r14"], &["r13"]);
}

/// In def-use analysis, `pop {r0, r1}` kills any earlier `r1` definition and supplies the new `r1`
/// value to subsequent reads instead of treating `r1` as an incoming parameter.
#[test]
fn thumb_pop_supplies_second_register_definition_in_def_use() {
    // pop {r0, r1}; mov r0, r1; bx lr
    let bytes = vec![0x03, 0xbc, 0x08, 0x46, 0x70, 0x47];
    let mut bin = Binary::raw(Arch::Arm, 0x1000, bytes.len() as u64, 0);
    bin.entry = Some(0x1001);
    let img = Image::from_binary(Arc::from(bytes), bin);
    let prog = Program::analyze(&img, &Limits::default(), &NoControl).unwrap();
    let f = prog.functions.get(&0x1000).unwrap();
    let flow = dataflow::def_use(&img, &prog, f, 100, 100);
    let pop = flow.nodes.iter().find(|n| n.addr == Some(0x1000)).unwrap();
    let mov = flow.nodes.iter().find(|n| n.addr == Some(0x1002)).unwrap();
    assert!(pop.defines.iter().any(|r| r == "r1"), "{pop:?}");
    assert!(flow.edges.iter().any(|e| e.from == pop.id && e.to == mov.id && e.register == "r1"), "{flow:?}");
    assert!(flow.parameters.is_empty(), "pop overwrote r0 and r1 before use: {flow:?}");
}
