use craft_anything_disasm::{Image, Limits, NoControl, Program, pseudo};
use craft_anything_formats::Arch;
use std::sync::Arc;

fn lift_a64(words: &[u32]) -> pseudo::Pseudo {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let image = Image::raw(Arc::from(bytes), Arch::Aarch64, 0x1000);
    let program = Program::analyze(&image, &Limits { scan_prologues: false, ..Limits::default() }, &NoControl).unwrap();
    let f = program.functions.get(&0x1000).unwrap();
    pseudo::lift(&image, &program, f, &|_| None)
}

/// `cmn w0, #1` (`adds wzr, w0, #1`) followed by `b.eq` lifts as `(w0 + 1) == 0` without
/// emitting a dead write to `wzr`.
#[test]
fn a64_cmn_immediate_lifts_sum_condition_without_writing_zero_register() {
    // cmn w0, #1; b.eq +0xc (0x1010); mov w0, #1; ret; mov w0, #2; ret
    let p = lift_a64(&[0x3100_041f, 0x5400_0060, 0x5280_0020, 0xd65f_03c0, 0x5280_0040, 0xd65f_03c0]);
    assert!(p.text.contains("(w0 + 1) == 0") || p.text.contains("(w0 + 1) != 0"), "{}", p.text);
    assert!(!p.text.contains("wzr"), "{}", p.text);
}

/// `cmn x0, x1, lsl #2` (`adds xzr, x0, x1, lsl #2`) lifts the shifted addend in the branch
/// condition without writing `xzr`.
#[test]
fn a64_cmn_shifted_register_lifts_sum_condition_without_writing_zero_register() {
    // cmn x0, x1, lsl #2; b.ne +0xc (0x1010); mov w0, #1; ret; mov w0, #2; ret
    let p = lift_a64(&[0xab01_081f, 0x5400_0061, 0x5280_0020, 0xd65f_03c0, 0x5280_0040, 0xd65f_03c0]);
    assert!(p.text.contains("(x0 + (x1 << 2))"), "{}", p.text);
    assert!(!p.text.contains("xzr"), "{}", p.text);
}

/// Discarded arithmetic/move results targeting `xzr`/`wzr` (like `add xzr, x0, x1` or `mov xzr, x0`)
/// do not emit bogus `xzr = ...;` statements, while flag-setting `adds` into a real register (`w0`)
/// keeps both its assignment and `w0 == 0` condition.
#[test]
fn a64_discarded_zero_writes_are_omitted_while_real_adds_keeps_its_register() {
    // add xzr, x0, x1; mov xzr, x0; adds w0, w1, #1; b.eq +0xc; mov w0, #1; ret; mov w0, #2; ret
    let p = lift_a64(&[0x8b01_001f, 0xaa00_03ff, 0x3100_0420, 0x5400_0060, 0x5280_0020, 0xd65f_03c0, 0x5280_0040, 0xd65f_03c0]);
    assert!(!p.text.contains("xzr ="), "{}", p.text);
    assert!(!p.text.contains("wzr"), "{}", p.text);
    assert!(p.text.contains("w0 = w1 + 1;"), "{}", p.text);
    assert!(p.text.contains("w0 == 0") || p.text.contains("w0 != 0"), "{}", p.text);
}

/// Existing `cmp` (`subs xzr, ...`) and `tst` (`ands xzr, ...`) controls continue to lift as before.
#[test]
fn a64_cmp_and_tst_controls_stay_unchanged() {
    // cmp w0, #1; b.eq +0xc; mov w0, #1; ret; mov w0, #2; ret
    let cmp_p = lift_a64(&[0x7100_041f, 0x5400_0060, 0x5280_0020, 0xd65f_03c0, 0x5280_0040, 0xd65f_03c0]);
    assert!(cmp_p.text.contains("w0 == 1") || cmp_p.text.contains("w0 != 1"), "{}", cmp_p.text);
    assert!(!cmp_p.text.contains("wzr"), "{}", cmp_p.text);

    // tst w0, #1; b.eq +0xc; mov w0, #1; ret; mov w0, #2; ret
    let tst_p = lift_a64(&[0x7200_001f, 0x5400_0060, 0x5280_0020, 0xd65f_03c0, 0x5280_0040, 0xd65f_03c0]);
    assert!(tst_p.text.contains("(w0 & 1) == 0") || tst_p.text.contains("(w0 & 1) != 0"), "{}", tst_p.text);
    assert!(!tst_p.text.contains("wzr"), "{}", tst_p.text);
}
