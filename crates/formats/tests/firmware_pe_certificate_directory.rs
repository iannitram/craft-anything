//! The firmware carver's PE region includes the certificate table only when the optional header
//! that declares it actually holds directory 4.

use craft_anything_formats::firmware::{ScanOptions, scan};

const LEN: usize = 0x2000;
const PE: usize = 0x40;
const COFF: usize = PE + 4;
const OPT: usize = COFF + 20;
const SIZE_OF_HEADERS: u32 = 0x200;

fn image(plus: bool, opt_size: u16, with_certificate: bool) -> Vec<u8> {
    let mut data = vec![0u8; LEN];
    data[..2].copy_from_slice(b"MZ");
    data[0x3c..0x40].copy_from_slice(&(PE as u32).to_le_bytes());
    data[PE..PE + 4].copy_from_slice(b"PE\0\0");
    data[COFF..COFF + 2].copy_from_slice(&0x14cu16.to_le_bytes());
    data[COFF + 2..COFF + 4].copy_from_slice(&0u16.to_le_bytes());
    data[COFF + 16..COFF + 18].copy_from_slice(&opt_size.to_le_bytes());
    data[OPT..OPT + 2].copy_from_slice(&(if plus { 0x20bu16 } else { 0x10b }).to_le_bytes());
    data[OPT + 60..OPT + 64].copy_from_slice(&SIZE_OF_HEADERS.to_le_bytes());
    if with_certificate {
        let fixed = if plus { 112 } else { 96 };
        let cert = OPT + fixed + 4 * 8;
        data[cert..cert + 4].copy_from_slice(&0x1000u32.to_le_bytes());
        data[cert + 4..cert + 8].copy_from_slice(&0x800u32.to_le_bytes());
    }
    data
}

fn pe_length(data: &[u8]) -> Option<u64> {
    let result = scan(data, &ScanOptions::default(), &|| false);
    result.regions.iter().find(|r| r.kind == "pe").expect("the PE is carved").length
}

#[test]
fn a_certificate_directory_outside_the_declared_optional_header_is_not_carved() {
    for plus in [false, true] {
        let fixed = if plus { 112 } else { 96 };
        let data = image(plus, fixed as u16, true);
        assert_eq!(pe_length(&data), Some(u64::from(SIZE_OF_HEADERS)), "plus={plus}: an undeclared certificate table is not part of the region");
    }
}

#[test]
fn a_declared_certificate_directory_still_extends_the_region() {
    for plus in [false, true] {
        let fixed = if plus { 112 } else { 96 };
        let data = image(plus, (fixed + 16 * 8) as u16, true);
        assert_eq!(pe_length(&data), Some(0x1800), "plus={plus}: a declared certificate table extends the region");
    }
}

#[test]
fn an_optional_header_shorter_than_the_fixed_fields_is_not_a_pe_region() {
    for plus in [false, true] {
        for opt_size in [0u16, 1, 70, 95] {
            let mut data = image(plus, opt_size, false);
            // Outside the declared header, and large enough to over-carve if it were read.
            data[OPT + 60..OPT + 64].copy_from_slice(&0x1800u32.to_le_bytes());
            let result = scan(&data, &ScanOptions::default(), &|| false);
            assert!(!result.regions.iter().any(|r| r.kind == "pe"), "plus={plus}, size={opt_size}: a PE without its declared fixed fields is not carved");
        }
    }
}
