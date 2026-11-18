//! Regressions for stale cross-references and quota consumption left behind when overran
//! functions are re-scanned after later function boundaries are discovered.

use craft_anything_disasm::{Image, Limits, NoControl, Program, XrefKind};
use craft_anything_formats::{Arch, Binary};
use std::sync::Arc;

/// Verify that a re-scanned switch function drops jump xrefs that pointed past the next function.
#[test]
fn switch_overrun_drops_stale_jump_xrefs_after_rescan() {
    // 0x1000: call 0x1080; jmp [rax*8 + 0x1100]      cases: 0x1010, 0x1011 (ret each)
    // 0x1080: 8 nops; ret                            table: 0x1010, 0x1011, 0x1085, 0x1088, 0
    let mut code = vec![0xcc; 0x140];
    code[..12].copy_from_slice(&[0xe8, 0x7b, 0, 0, 0, 0xff, 0x24, 0xc5, 0x00, 0x11, 0, 0]);
    code[0x10..0x12].copy_from_slice(&[0xc3, 0xc3]);
    code[0x80..0x89].copy_from_slice(&[0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0x90, 0xc3]);
    for (i, t) in [0x1010u64, 0x1011, 0x1085, 0x1088, 0].iter().enumerate() {
        code[0x100 + i * 8..0x108 + i * 8].copy_from_slice(&t.to_le_bytes());
    }
    let raw = Image::raw(Arc::from(code), Arch::X86_64, 0x1000);
    let p = Program::analyze(&raw, &Limits::default(), &NoControl).unwrap();

    let caller = p.functions.get(&0x1000).unwrap();
    assert_eq!(caller.jump_tables.first().map(|t| t.targets.clone()), Some(vec![0x1010, 0x1011]));

    let outgoing: Vec<u64> = p.xrefs_from.get(&0x1005).map(|xs| xs.iter().map(|x| x.to).collect()).unwrap_or_default();
    assert_eq!(outgoing, vec![0x1010, 0x1011], "stale jump targets remained in xrefs_from: {outgoing:#x?}");
    assert!(!p.xrefs_to.contains_key(&0x1085), "stale xref into 0x1085: {:?}", p.xrefs_to.get(&0x1085));
    assert!(!p.xrefs_to.contains_key(&0x1088), "stale xref into 0x1088: {:?}", p.xrefs_to.get(&0x1088));
    assert!(p.callers(0x1085).is_empty(), "stale caller remained for 0x1085: {:?}", p.callers(0x1085));
}

/// Verify that stale xrefs emitted inside unreachable code picked up only by an overran switch
/// scan are removed after trimming, including calls and data references.
#[test]
fn trimmed_overrun_block_drops_call_and_data_xrefs() {
    // 0x1000: call 0x1080; jmp [rax*8 + 0x1100]      cases: 0x1010, 0x1011 (ret)
    // 0x1080: ret                                    (one byte: body stops at 0x1081)
    // 0x1090: lea rdi, [rip+0x19] (0x10b0); ret      (only reachable if leaked switch table runs past 0x1080)
    // 0x10b0: "HelloWorld"
    // table at 0x1100: [0x1010, 0x1011, 0x1090, 0]
    let mut code = vec![0xcc; 0x140];
    code[..12].copy_from_slice(&[0xe8, 0x7b, 0, 0, 0, 0xff, 0x24, 0xc5, 0x00, 0x11, 0, 0]);
    code[0x10..0x12].copy_from_slice(&[0xc3, 0xc3]);
    code[0x80] = 0xc3;
    // 0x1090: lea rdi, [rip + 0x19] -> rip is 0x1097, target 0x10b0; ret
    code[0x90..0x98].copy_from_slice(&[0x48, 0x8d, 0x3d, 0x19, 0x00, 0x00, 0x00, 0xc3]);
    code[0xb0..0xba].copy_from_slice(b"HelloWorld");
    for (i, t) in [0x1010u64, 0x1011, 0x1090, 0].iter().enumerate() {
        code[0x100 + i * 8..0x108 + i * 8].copy_from_slice(&t.to_le_bytes());
    }
    let raw = Image::raw(Arc::from(code), Arch::X86_64, 0x1000);
    let p = Program::analyze(&raw, &Limits::default(), &NoControl).unwrap();

    assert!(p.function_containing(0x1090).is_none(), "0x1090 should not stay attached to any function");
    assert!(!p.xrefs_from.contains_key(&0x1090), "stale data/string xref from dropped overrun block: {:?}", p.xrefs_from.get(&0x1090));
    assert!(!p.xrefs_to.contains_key(&0x10b0), "stale string xref target: {:?}", p.xrefs_to.get(&0x10b0));
    assert!(!p.xrefs_to.contains_key(&0x1090), "stale jump xref into 0x1090: {:?}", p.xrefs_to.get(&0x1090));
}

/// Verify that stale xrefs produced before an overran re-scan do not exhaust `limits.max_xrefs`.
#[test]
fn overran_rescan_frees_xref_budget_for_other_functions() {
    // 0x1000: call 0x1080; jmp [rax*8 + 0x1100]      valid cases: 0x1010, 0x1011 (3 final xrefs: 1 call + 2 jumps)
    // 0x1080: call 0x10c0; ret                       (1 call xref)
    // 0x10c0: ret
    // table at 0x1100: [0x1010, 0x1011, 0x1080, 0x1085, 0x1090, 0x10a0, 0]
    // First pass on 0x1000 emits 7 xrefs (1 call + 6 switch jumps) before re-scan trims 0x1000 to 3 xrefs.
    // With max_xrefs = 4, the caller keeps its 3 trimmed xrefs and 0x1080 still keeps its call xref.
    let mut code = vec![0xcc; 0x140];
    code[..12].copy_from_slice(&[0xe8, 0x7b, 0, 0, 0, 0xff, 0x24, 0xc5, 0x00, 0x11, 0, 0]);
    code[0x10..0x12].copy_from_slice(&[0xc3, 0xc3]);
    // 0x1080: call 0x10c0; ret
    code[0x80..0x86].copy_from_slice(&[0xe8, 0x3b, 0x00, 0x00, 0x00, 0xc3]);
    code[0x86..0xa8].fill(0x90);
    code[0xa8] = 0xc3;
    code[0xc0] = 0xc3;
    for (i, t) in [0x1010u64, 0x1011, 0x1080, 0x1085, 0x1090, 0x10a0, 0].iter().enumerate() {
        code[0x100 + i * 8..0x108 + i * 8].copy_from_slice(&t.to_le_bytes());
    }
    let raw = Image::raw(Arc::from(code), Arch::X86_64, 0x1000);
    let limits = Limits { max_xrefs: 4, ..Limits::default() };
    let p = Program::analyze(&raw, &limits, &NoControl).unwrap();

    let callee_call = p.xrefs_from.get(&0x1080).and_then(|xs| xs.iter().find(|x| x.kind == XrefKind::Call && x.to == 0x10c0));
    assert!(callee_call.is_some(), "0x1080 lost its call xref because stale overrun xrefs spent the budget: {:?}", p.xrefs_from);
    assert_eq!(p.stats.xrefs, 4);
}

/// Verify that under a tight global `max_xrefs` budget, cross-references are kept in function
/// discovery order rather than sorted by address (so a higher-address entry discovered first
/// keeps priority over a lower-address callee discovered from it).
#[test]
fn tight_xref_budget_preserves_discovery_order_across_addresses() {
    // 0x1080 (entry, discovered 1st): call 0x1020; call 0x10c0; ret   (2 xrefs)
    // 0x1020 (callee, discovered 2nd, lower address): call 0x10c0; call 0x10e0; ret   (2 xrefs)
    // 0x10c0: ret, 0x10e0: ret
    let mut code = vec![0xcc; 0x100];
    // 0x1020: call +0x9b (0x10c0); call +0xb6 (0x10e0); ret
    code[0x20..0x2b].copy_from_slice(&[0xe8, 0x9b, 0x00, 0x00, 0x00, 0xe8, 0xb6, 0x00, 0x00, 0x00, 0xc3]);
    // 0x1080: call -0x65 (0x1020); call +0x36 (0x10c0); ret
    code[0x80..0x8b].copy_from_slice(&[0xe8, 0x9b, 0xff, 0xff, 0xff, 0xe8, 0x36, 0x00, 0x00, 0x00, 0xc3]);
    code[0xc0] = 0xc3;
    code[0xe0] = 0xc3;

    let mut bin = Binary::raw(Arch::X86_64, 0x1000, code.len() as u64, 0);
    bin.entry = Some(0x1080);
    let img = Image::from_binary(Arc::from(code), bin);
    let limits = Limits { max_xrefs: 3, ..Limits::default() };
    let p = Program::analyze(&img, &limits, &NoControl).unwrap();

    // Entry (0x1080, discovered first) must keep both of its calls; 0x1020 keeps only its first.
    assert!(p.xrefs_from.contains_key(&0x1080), "missing entry first call: {:?}", p.xrefs_from);
    assert!(p.xrefs_from.contains_key(&0x1085), "missing entry second call: {:?}", p.xrefs_from);
    assert!(p.xrefs_from.contains_key(&0x1020), "missing callee first call: {:?}", p.xrefs_from);
    assert!(!p.xrefs_from.contains_key(&0x1025), "lower-address callee exceeded remaining budget: {:?}", p.xrefs_from);
    assert_eq!(p.stats.xrefs, 3);
}

/// Verify that when both A and C overrun while earlier B was clipped under `max_xrefs = 4`,
/// reconciling rescans and refills in one discovery-order pass grants B freed capacity before C.
#[test]
fn overran_rescan_and_refill_share_one_discovery_order_pass() {
    // Discovery order: A (0x1000) -> B (0x1080) -> C (0x1100)
    // Pass 1: A emits 2 calls (B, C) + 3 overrun switch jumps (0x1090..=0x1092 >= B=0x1080), taking all 4.
    //         B (0x1080) emits 2 calls (D=0x11c0, E=0x11d0), clipped to 0.
    //         C (0x1100) emits 2 calls (F=0x1180, G=0x1190) + 1 overrun switch jump (0x1195 >= F=0x1180), clipped to 0.
    // Single discovery-order pass after clearing overran A & C:
    //   A keeps 2 calls; B refills 2 calls (budget now full at 4); C is trimmed with 0 xrefs.
    let mut code = vec![0xcc; 0x300];
    // A at 0x1000: call 0x1080; call 0x1100; jmp [rax*8 + 0x1200]
    code[0x00..0x11].copy_from_slice(&[
        0xe8, 0x7b, 0x00, 0x00, 0x00, // call 0x1080
        0xe8, 0xf6, 0x00, 0x00, 0x00, // call 0x1100
        0xff, 0x24, 0xc5, 0x00, 0x12, 0x00, 0x00, // jmp [rax*8 + 0x1200]
    ]);
    for (i, t) in [0x1090u64, 0x1091, 0x1092, 0].iter().enumerate() {
        code[0x200 + i * 8..0x208 + i * 8].copy_from_slice(&t.to_le_bytes());
    }
    code[0x90..=0x92].fill(0xc3);

    // B at 0x1080: call 0x11c0; call 0x11d0; ret
    code[0x80..0x8b].copy_from_slice(&[
        0xe8, 0x3b, 0x01, 0x00, 0x00, // call 0x11c0
        0xe8, 0x46, 0x01, 0x00, 0x00, // call 0x11d0
        0xc3, // ret
    ]);

    // C at 0x1100: call 0x1180; call 0x1190; jmp [rax*8 + 0x1240]
    code[0x100..0x111].copy_from_slice(&[
        0xe8, 0x7b, 0x00, 0x00, 0x00, // call 0x1180
        0xe8, 0x86, 0x00, 0x00, 0x00, // call 0x1190
        0xff, 0x24, 0xc5, 0x40, 0x12, 0x00, 0x00, // jmp [rax*8 + 0x1240]
    ]);
    for (i, t) in [0x1195u64, 0].iter().enumerate() {
        code[0x240 + i * 8..0x248 + i * 8].copy_from_slice(&t.to_le_bytes());
    }

    for off in [0x180usize, 0x190, 0x195, 0x1c0, 0x1d0] {
        code[off] = 0xc3;
    }

    let raw = Image::raw(Arc::from(code), Arch::X86_64, 0x1000);
    let limits = Limits { max_xrefs: 4, ..Limits::default() };
    let p = Program::analyze(&raw, &limits, &NoControl).unwrap();

    assert!(p.xrefs_from.contains_key(&0x1000) && p.xrefs_from.contains_key(&0x1005), "A calls missing: {:?}", p.xrefs_from);
    assert!(p.xrefs_from.contains_key(&0x1080) && p.xrefs_from.contains_key(&0x1085), "B must refill before later overran C: {:?}", p.xrefs_from);
    assert!(!p.xrefs_from.contains_key(&0x1100) && !p.xrefs_from.contains_key(&0x1105), "C jumped ahead of B: {:?}", p.xrefs_from);
    assert!(p.functions.get(&0x1100).unwrap().jump_tables.is_empty(), "C's leaked switch target should still be trimmed");
    assert_eq!(p.stats.xrefs, 4);
}
