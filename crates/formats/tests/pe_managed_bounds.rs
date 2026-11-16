use craft_anything_formats::pe_is_managed;

/// Build a PE whose CLR directory bytes exist, independently of the declared header size.
fn pe(plus: bool, size: u16, count: u32) -> Vec<u8> {
    let mut b = vec![0; 512];
    b[..2].copy_from_slice(b"MZ");
    b[0x3c..0x40].copy_from_slice(&64u32.to_le_bytes());
    b[64..68].copy_from_slice(b"PE\0\0");
    b[84..86].copy_from_slice(&size.to_le_bytes());
    b[88..90].copy_from_slice(&(if plus { 0x20bu16 } else { 0x10b }).to_le_bytes());
    let dirs = 88 + if plus { 112 } else { 96 };
    b[dirs - 4..dirs].copy_from_slice(&count.to_le_bytes());
    b[dirs + 112..dirs + 116].copy_from_slice(&4096u32.to_le_bytes());
    b[dirs + 116..dirs + 120].copy_from_slice(&72u32.to_le_bytes());
    b
}

/// Bytes after the optional header must not identify native files as managed.
#[test]
fn ignores_clr_bytes_outside_optional_header() {
    for plus in [false, true] {
        let end = if plus { 232 } else { 216 };
        for size in [0, 2, end - 8, end - 1] {
            assert!(!pe_is_managed(&pe(plus, size, 16)), "plus={plus}, size={size}");
        }
    }
}

/// The complete last directory is enough, without requiring an unused sixteenth entry.
#[test]
fn accepts_exact_clr_directory_boundary() {
    for plus in [false, true] {
        assert!(pe_is_managed(&pe(plus, if plus { 232 } else { 216 }, 15)));
        assert!(!pe_is_managed(&pe(plus, 240, 14)));
    }
}

/// Truncating either half of the directory never fabricates a managed image.
#[test]
fn rejects_truncated_clr_directory() {
    for plus in [false, true] {
        let b = pe(plus, 240, 16);
        let end = 88 + if plus { 232 } else { 216 };
        for cut in end - 8..end {
            assert!(!pe_is_managed(&b[..cut]));
        }
    }
}
