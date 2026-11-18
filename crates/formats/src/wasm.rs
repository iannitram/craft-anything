//! Shared WebAssembly inspection; this module preserves the app error contract.
use crate::FormatError;
pub use craft_wasm::{
    GlueScan, MAX_INPUT, MAX_ITEMS, ProducerField, ProducerValue, WasmCustom, WasmExport, WasmFunction, WasmGlobal, WasmImport, WasmMemory, WasmReport,
    WasmSection, WasmTable, WasmTag, WasmValidation, is_wasm, scan_glue, to_wat, validate,
};

pub fn inspect(data: &[u8]) -> Result<WasmReport, FormatError> {
    craft_wasm::inspect(data).map_err(|error| match error {
        craft_wasm::WasmError::Malformed(message) => FormatError::Malformed(message),
        craft_wasm::WasmError::TooLarge(message) => FormatError::TooLarge(message),
    })
}

#[cfg(test)]
#[path = "wasm_legacy.rs"]
mod legacy;

#[cfg(test)]
mod parity {
    use super::*;
    #[test]
    fn shared_reports_errors_and_text_match() {
        let sample = craft_anything_testkit::formats::wasm_module();
        let mut samples = craft_anything_testkit::mutations(&sample);
        samples.push(sample);
        samples.push(b"not wasm".to_vec());
        samples.push(b"\0asm\x0d\0\x01\0".to_vec());
        for data in samples {
            assert_eq!(is_wasm(&data), legacy::is_wasm(&data));
            assert_eq!(serde_json::to_value(validate(&data)).unwrap(), serde_json::to_value(legacy::validate(&data)).unwrap());
            match (legacy::inspect(&data), inspect(&data)) {
                (Ok(old), Ok(new)) => {
                    assert_eq!(serde_json::to_value(&old).unwrap(), serde_json::to_value(&new).unwrap());
                    let glue = craft_anything_testkit::formats::WASM_GLUE;
                    assert_eq!(
                        serde_json::to_value(legacy::scan_glue(glue, &old.imports)).unwrap(),
                        serde_json::to_value(scan_glue(glue, &new.imports)).unwrap()
                    );
                    for cap in [0, 1, 40, 4096] {
                        assert_eq!(legacy::to_wat(&data, cap), to_wat(&data, cap));
                    }
                }
                (Err(old), Err(new)) => assert_eq!(old, new),
                _ => panic!("WASM parity mismatch"),
            }
        }
    }
    fn section(module: &mut Vec<u8>, id: u8, body: &[u8]) {
        fn leb(out: &mut Vec<u8>, mut value: usize) {
            loop {
                let byte = (value & 0x7f) as u8;
                value >>= 7;
                out.push(byte | if value == 0 { 0 } else { 0x80 });
                if value == 0 {
                    break;
                }
            }
        }
        module.push(id);
        leb(module, body.len());
        module.extend_from_slice(body);
    }

    fn same_report(bytes: &[u8]) -> WasmReport {
        let old = legacy::inspect(bytes).unwrap();
        let new = inspect(bytes).unwrap();
        assert_eq!(serde_json::to_value(old).unwrap(), serde_json::to_value(&new).unwrap());
        new
    }

    #[test]
    fn imported_and_defined_resources_preserve_indexes_and_signatures() {
        let mut bytes = b"\0asm\x01\0\0\0".to_vec();
        section(&mut bytes, 1, &[1, 0x60, 0, 0]);
        // env.t table, env.m memory, env.g mutable i32 global, env.e exception tag.
        section(
            &mut bytes,
            2,
            &[
                4, 3, b'e', b'n', b'v', 1, b't', 1, 0x70, 0, 1, 3, b'e', b'n', b'v', 1, b'm', 2, 0, 1, 3, b'e', b'n', b'v', 1, b'g', 3, 0x7f, 1, 3, b'e', b'n',
                b'v', 1, b'e', 4, 0, 0,
            ],
        );
        section(&mut bytes, 3, &[1, 0]);
        section(&mut bytes, 4, &[1, 0x70, 0, 1]);
        section(&mut bytes, 5, &[1, 0, 1]);
        section(&mut bytes, 13, &[1, 0, 0]);
        section(&mut bytes, 6, &[1, 0x7f, 0, 0x41, 0, 0x0b]);
        section(&mut bytes, 7, &[5, 1, b't', 1, 1, 1, b'm', 2, 1, 1, b'g', 3, 1, 1, b'e', 4, 1, 1, b'f', 0, 0]);
        section(&mut bytes, 8, &[0]);
        section(&mut bytes, 9, &[0]);
        section(&mut bytes, 12, &[0]);
        section(&mut bytes, 10, &[1, 2, 0, 0x0b]);
        section(&mut bytes, 11, &[0]);
        let report = same_report(&bytes);
        assert!(report.validation.valid, "{:?}", report.validation);
        assert_eq!(report.imports.len(), 4);
        assert_eq!(report.tables.iter().map(|x| (x.index, x.imported)).collect::<Vec<_>>(), [(0, true), (1, false)]);
        assert_eq!(report.memories.iter().map(|x| (x.index, x.imported)).collect::<Vec<_>>(), [(0, true), (1, false)]);
        assert_eq!(report.globals.iter().map(|x| (x.index, x.imported)).collect::<Vec<_>>(), [(0, true), (1, false)]);
        assert_eq!(report.tags.iter().map(|x| (x.index, x.imported)).collect::<Vec<_>>(), [(0, true), (1, false)]);
        assert_eq!(report.start_function, Some(0));
        assert_eq!(report.data_count, Some(0));
        for damaged in craft_anything_testkit::mutations(&bytes) {
            if is_wasm(&damaged) {
                same_report(&damaged);
            }
        }
    }

    #[test]
    fn component_layout_and_truncated_sections_preserve_partial_reports() {
        let mut bytes = b"\0asm\x0d\0\x01\0".to_vec();
        for id in 1..=11 {
            section(&mut bytes, id, &[]);
        }
        section(&mut bytes, 0, &[1, b'x']);
        let report = same_report(&bytes);
        assert_eq!(report.encoding, "component");
        assert_eq!(report.sections.len(), 12);
        assert_eq!(report.sections[0].name, "core module");
        assert_eq!(report.sections[11].name, "custom:x");
        for end in 8..bytes.len() {
            let truncated = same_report(&bytes[..end]);
            assert_eq!(truncated.encoding, "component");
        }
    }
}
