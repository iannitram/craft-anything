//! The .NET reader bounds its optional-header fields by the size the COFF header declares.
//! `pe::headers` already did; `dotnet::pe::parse_pe` read the CLI data directory from section-table
//! bytes when the declared header ended before it.

use craft_anything_formats::FormatError;
use craft_anything_formats::dotnet::pe;
use craft_anything_testkit::managed as fx;

/// The testkit's PE32 optional header starts at 0x98 and declares 224 bytes, with its only section
/// header just past it.
const OPT: usize = 0x98;
const DECLARED_SIZE: usize = 0x94;
const SECTION: usize = OPT + 224;

#[test]
fn the_cli_directory_must_lie_inside_the_declared_optional_header() {
    let mut data = fx::managed_assembly(1);
    let section = data[SECTION..SECTION + 40].to_vec();
    // The declared header stops after the PE32 fixed fields. Moving the section table there makes
    // the old reader see a valid section, while directory 14 is still read from bytes past the end.
    data[DECLARED_SIZE..DECLARED_SIZE + 2].copy_from_slice(&96u16.to_le_bytes());
    data[OPT + 96..OPT + 96 + 40].copy_from_slice(&section);
    let cli = OPT + 96 + 14 * 8;
    assert!(data[cli..cli + 8].iter().any(|&b| b != 0), "the bytes directory 14 would read are still non-zero");
    assert!(matches!(pe::parse_pe(&data), Err(FormatError::Truncated(_))));
}

#[test]
fn an_optional_header_too_short_for_its_fixed_fields_is_refused() {
    for size in [0u16, 2, 70, 95] {
        let mut data = fx::managed_assembly(1);
        data[DECLARED_SIZE..DECLARED_SIZE + 2].copy_from_slice(&size.to_le_bytes());
        assert!(matches!(pe::parse_pe(&data), Err(FormatError::Truncated(_))), "size {size}");
    }
}

#[test]
fn a_valid_managed_assembly_still_parses() {
    let image = pe::parse_pe(&fx::managed_assembly(1)).unwrap();
    assert_eq!(image.optional_magic, 0x10b);
    assert!(!image.cli_directory.is_empty());
}
