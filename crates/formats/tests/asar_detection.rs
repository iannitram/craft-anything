use craft_anything_formats::asar;

/// An ASAR whose header JSON is `json`, laid out as `asar::build` lays it out.
fn archive(json: &[u8]) -> Vec<u8> {
    let pad = (4 - json.len() % 4) % 4;
    let payload = json.len() + 4 + pad;
    let mut out = Vec::new();
    for word in [4, payload + 4, payload, json.len()] {
        out.extend_from_slice(&(word as u32).to_le_bytes());
    }
    out.extend_from_slice(json);
    out.extend(std::iter::repeat_n(0u8, pad));
    out
}

#[test]
fn every_header_the_reader_accepts_is_recognized() {
    for json in [
        br#"{"files":{"a.js":{"size":0,"offset":"0"}}}"#.as_slice(),
        br#"{ "files": {"a.js": {"size": 0, "offset": "0"}}}"#,
        br#"{"version":1,"files":{"a.js":{"size":0,"offset":"0"}}}"#,
        b"{\n  \"files\": {}\n}",
    ] {
        let bytes = archive(json);
        assert!(asar::parse(&bytes, 10).is_ok(), "{}", String::from_utf8_lossy(json));
        assert!(craft_anything_formats::looks_like_asar(&bytes), "{}", String::from_utf8_lossy(json));
        assert!(asar::looks_like_asar(&bytes), "{}", String::from_utf8_lossy(json));
    }
}

#[test]
fn the_head_of_a_file_is_enough() {
    // `js_app_map` passes only the first bytes of a large file.
    let good = archive(br#"{ "files": {}}"#);
    assert!(craft_anything_formats::looks_like_asar(&good[..17]));
    assert!(!craft_anything_formats::looks_like_asar(&good[..16]));
}
