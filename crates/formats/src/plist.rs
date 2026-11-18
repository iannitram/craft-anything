//! Property lists: binary (`bplist00`) and XML, parsed into one [`Value`] tree and projected to
//! normalised JSON. Bounded: object counts, nesting depth, total nodes and copied data and text
//! are capped; shared references in binary plists are expanded at most once per path (cycles are
//! refused).

use crate::FormatError;
use serde_json::{Map, Value as Json, json};

/// Deepest nesting accepted.
pub const MAX_DEPTH: usize = 256;
/// Most nodes produced by one parse (guards against reference blow-up in binary plists).
pub const MAX_NODES: usize = 2_000_000;
/// Most data and decoded text bytes one binary plist may produce, unless three times the file size
/// is more (damaged text decodes to at most three bytes per stored byte, so a file whose objects
/// are each used once always fits). A shared object is copied once per reference, so repeats spend
/// this budget.
pub const MAX_EXPANDED_BYTES: usize = 64 << 20;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Dict(Vec<(String, Value)>),
    Array(Vec<Value>),
    String(String),
    Integer(i128),
    Real(f64),
    Bool(bool),
    /// Seconds since 2001-01-01T00:00:00Z (binary), or the ISO text (XML).
    Date(DateValue),
    Data(Vec<u8>),
    /// A keyed-archive object reference.
    Uid(u64),
    Null,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DateValue {
    Seconds(f64),
    Text(String),
}

impl Value {
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Dict(d) => d.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_uid(&self) -> Option<u64> {
        match self {
            Value::Uid(u) => Some(*u),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i128> {
        match self {
            Value::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// Normalised JSON: dicts keep key order, data becomes `{"$data": base64}`, dates
    /// `{"$date": iso}`, UIDs `{"$uid": n}`.
    pub fn to_json(&self) -> Json {
        self.to_json_depth(0)
    }

    fn to_json_depth(&self, depth: usize) -> Json {
        if depth > MAX_DEPTH {
            return json!({"$truncated": "too deep"});
        }
        match self {
            Value::Dict(d) => {
                let mut m = Map::new();
                for (k, v) in d {
                    m.insert(k.clone(), v.to_json_depth(depth + 1));
                }
                Json::Object(m)
            }
            Value::Array(a) => Json::Array(a.iter().map(|v| v.to_json_depth(depth + 1)).collect()),
            Value::String(s) => Json::String(s.clone()),
            Value::Integer(i) => int_json(*i),
            Value::Real(f) => serde_json::Number::from_f64(*f).map(Json::Number).unwrap_or_else(|| Json::String(f.to_string())),
            Value::Bool(b) => Json::Bool(*b),
            Value::Date(DateValue::Seconds(s)) => json!({"$date": apple_date_iso(*s)}),
            Value::Date(DateValue::Text(t)) => json!({"$date": t}),
            Value::Data(d) => json!({"$data": base64(d), "length": d.len()}),
            Value::Uid(u) => json!({"$uid": u}),
            Value::Null => Json::Null,
        }
    }
}

pub fn int_json(i: i128) -> Json {
    if let Ok(v) = i64::try_from(i) {
        Json::from(v)
    } else if let Ok(v) = u64::try_from(i) {
        Json::from(v)
    } else {
        Json::String(i.to_string())
    }
}

/// Standard base64 with padding.
pub use craft_mime::transfer::base64;

/// Decode base64, ignoring whitespace; `None` on invalid characters.
pub fn base64_decode(text: &str) -> Option<Vec<u8>> {
    crate::bytes::decode_base64(text, false)
}

/// Maximum absolute Apple-epoch seconds (`1e13`, ~317,000 years from 2001) formatted as ISO-8601.
/// Fits losslessly inside `f64`'s 53-bit integer significand; larger magnitudes stay raw numbers.
const MAX_APPLE_DATE_SECS: i128 = 10_000_000_000_000;

/// An Apple absolute time (seconds since 2001-01-01Z) as ISO-8601 UTC, rounded to microseconds.
pub fn apple_date_iso(secs: f64) -> String {
    if !secs.is_finite() || secs.abs() > MAX_APPLE_DATE_SECS as f64 {
        return format!("{secs}");
    }
    // Separate the fraction before adding the epoch offset, which would otherwise lose
    // precision for small intervals. Microseconds avoid displaying the floating-point noise
    // of contemporary dates as nanosecond precision. Floor keeps pre-epoch fractions in the
    // preceding second.
    let whole = secs.floor();
    let mut micros = ((secs - whole) * 1_000_000.0).round() as u32;
    let mut unix = whole as i64 + 978_307_200;
    if micros == 1_000_000 {
        unix += 1;
        micros = 0;
    }
    let days = unix.div_euclid(86_400);
    let rem = unix.rem_euclid(86_400);
    // Civil date from days since 1970-01-01.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let fraction = if micros == 0 { String::new() } else { format!(".{micros:06}").trim_end_matches('0').to_string() };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}{fraction}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

/// Format an integer Apple absolute time using [`apple_date_iso`] within [`MAX_APPLE_DATE_SECS`],
/// falling back to the exact decimal string when outside that range.
pub(crate) fn apple_date_int_iso(secs: i128) -> String {
    if secs.unsigned_abs() <= MAX_APPLE_DATE_SECS as u128 { apple_date_iso(secs as f64) } else { secs.to_string() }
}

/// True when the bytes look like a property list of either kind.
pub fn looks_like_plist(data: &[u8]) -> bool {
    if data.len() >= 8 && data.starts_with(b"bplist0") {
        return true;
    }
    let prefix = &data[..data.len().min(1024)];
    let (body, marker): (&[u8], &[u8]) = if let Some(body) = prefix.strip_prefix(&[0xff, 0xfe]) {
        (body, b"<\0p\0l\0i\0s\0t\0")
    } else if let Some(body) = prefix.strip_prefix(&[0xfe, 0xff]) {
        (body, b"\0<\0p\0l\0i\0s\0t")
    } else {
        return prefix.windows(6).any(|w| w == b"<plist");
    };
    // XML detection must agree with parse_xml's BOM decoding, including code-unit alignment.
    body.windows(marker.len()).step_by(2).any(|w| w == marker)
}

/// Parse a binary or XML property list.
pub fn parse(data: &[u8]) -> Result<Value, FormatError> {
    if data.starts_with(b"bplist") { parse_binary(data) } else { parse_xml(data) }
}

// ---------------------------------------------------------------------------------------------
// Binary

/// Parse a binary plist with the historical bplist0x version admission policy.
pub fn parse_binary(data: &[u8]) -> Result<Value, FormatError> {
    parse_binary_within(data, MAX_EXPANDED_BYTES.max(data.len().saturating_mul(3)))
}

/// Preserve the app's input/object, expanded-node, depth and copied-text/data budgets.
fn parse_binary_within(data: &[u8], byte_budget: usize) -> Result<Value, FormatError> {
    let limits = craft_plist::Limits {
        max_input_bytes: data.len(),
        // The former reader rejected an object count larger than the encoded file.
        max_objects: data.len(),
        max_nodes: MAX_NODES,
        max_depth: MAX_DEPTH,
        max_expanded_bytes: byte_budget,
    };
    let options = craft_plist::BinaryOptions { allow_unknown_minor_version: true };
    craft_plist::parse_binary_with_options(data, limits, options).map(shared_value).map_err(|error| match error {
        craft_plist::FormatError::TooLarge(message) if message == "expanded node count" => FormatError::TooLarge(format!("more than {MAX_NODES} plist nodes")),
        other => shared_error(other),
    })
}

// ---------------------------------------------------------------------------------------------
// XML

/// Parse an XML property list with the shared reader. The app's normalized JSON
/// projection remains local; the shared tree is moved, not cloned.
pub fn parse_xml(data: &[u8]) -> Result<Value, FormatError> {
    let limits = craft_plist::Limits {
        max_input_bytes: data.len(),
        max_objects: MAX_NODES,
        max_nodes: MAX_NODES,
        max_depth: MAX_DEPTH,
        // UTF-16 to UTF-8 expansion fits three times the encoded input. The
        // shared reader also charges decoded leaf/key text against this budget.
        max_expanded_bytes: MAX_EXPANDED_BYTES.max(data.len().saturating_mul(3)),
    };
    // The old tokenizer could append at most two tokens after its MAX_NODES check.
    craft_plist::parse_xml_with_token_limit(data, limits, craft_plist::XmlOptions::default(), MAX_NODES.saturating_add(2))
        .map(shared_value)
        .map_err(shared_error)
}

fn shared_error(error: craft_plist::FormatError) -> FormatError {
    match error {
        craft_plist::FormatError::Malformed(message) => FormatError::Malformed(message),
        craft_plist::FormatError::Truncated(message) => FormatError::Truncated(message),
        craft_plist::FormatError::Unsupported(message) => FormatError::Unsupported(message),
        craft_plist::FormatError::TooLarge(message) => FormatError::TooLarge(message),
    }
}

// Recursion is bounded by the shared reader's MAX_DEPTH limit before conversion.
fn shared_value(value: craft_plist::Value) -> Value {
    use craft_plist::Value as Shared;
    match value {
        Shared::Dict(entries) => Value::Dict(entries.into_iter().map(|(key, value)| (key, shared_value(value))).collect()),
        Shared::Array(values) => Value::Array(values.into_iter().map(shared_value).collect()),
        Shared::String(text) => Value::String(text),
        Shared::Integer(value) => Value::Integer(value),
        Shared::Real(value) => Value::Real(value),
        Shared::Bool(value) => Value::Bool(value),
        Shared::Date(craft_plist::DateValue::Seconds(value)) => Value::Date(DateValue::Seconds(value)),
        Shared::Date(craft_plist::DateValue::Text(value)) => Value::Date(DateValue::Text(value)),
        Shared::Data(data) => Value::Data(data),
        Shared::Uid(value) => Value::Uid(value),
        Shared::Null => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft_anything_testkit::apple as fx;

    #[test]
    fn shared_xml_keeps_order_dates_and_rejects_nonfinite_reals() {
        let value = parse_xml(br#"<plist><dict><key>x</key><integer>1</integer><key>x</key><integer>2</integer><key>when</key><date> 2026-01-01T00:00:00Z </date></dict></plist>"#).unwrap();
        let Value::Dict(entries) = value else { panic!("expected dictionary") };
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0], ("x".into(), Value::Integer(1)));
        assert_eq!(entries[1], ("x".into(), Value::Integer(2)));
        assert_eq!(entries[2].1, Value::Date(DateValue::Text("2026-01-01T00:00:00Z".into())));
        for real in ["NaN", "inf", "-inf", "1e999"] {
            let xml = format!("<plist><real>{real}</real></plist>");
            assert!(matches!(parse_xml(xml.as_bytes()), Err(FormatError::Malformed(_))));
        }
    }

    #[test]
    fn shared_xml_preserves_app_depth_allowance() {
        let xml = format!("{}<string>x</string>{}", "<array>".repeat(80), "</array>".repeat(80));
        assert!(parse_xml(xml.as_bytes()).is_ok());
        let deep = format!("{}<string>x</string>{}", "<array>".repeat(MAX_DEPTH + 1), "</array>".repeat(MAX_DEPTH + 1));
        assert!(matches!(parse_xml(deep.as_bytes()), Err(FormatError::TooLarge(_))));
    }

    #[test]
    fn xml_has_one_complete_root_with_only_markup_space_around_values() {
        for invalid in [
            "<plist><true/>",
            "<plist><true/></dict>",
            "<plist><true/><false/></plist>",
            "<plist><true/></plist><dict/>",
            "<false/>junk",
            "text<true/>",
            "<array>junk<true/></array>",
            "<dict><key>x</key>garbage<true/></dict>",
            "<true>no</true>",
            "<false>yes</false>",
            "<array><key/></array>",
        ] {
            assert!(parse_xml(invalid.as_bytes()).is_err(), "{invalid}");
        }
        let xml = b"<?example content > <false/> ?>\n<plist><dict><key/><string>  </string></dict></plist><!-- end -->";
        assert_eq!(parse_xml(xml).unwrap().get("").and_then(Value::as_str), Some("  "));
        assert_eq!(parse_xml(b" <true></true> ").unwrap(), Value::Bool(true));
    }

    #[test]
    fn xml_rejects_partial_code_units_and_preserves_the_utf8_bom() {
        assert_eq!(parse_xml(b"\xef\xbb\xbf<string>ok</string>").unwrap().as_str(), Some("ok"));
        assert!(parse_xml(b"<string>\xff</string>").is_err());
        for be in [false, true] {
            let mut bytes = if be { vec![0xfe, 0xff] } else { vec![0xff, 0xfe] };
            for unit in "<true/>".encode_utf16() {
                bytes.extend(if be { unit.to_be_bytes() } else { unit.to_le_bytes() });
            }
            bytes.push(0);
            assert!(parse_xml(&bytes).is_err());
        }
    }

    #[test]
    fn a_doctype_internal_subset_is_skipped_as_one_declaration() {
        let doc = br#"<!DOCTYPE plist [<!ELEMENT plist (string)><!ENTITY text "x]>y"><!-- ]> --><?note ]> ?>]><plist><string>ok</string></plist>"#;
        assert_eq!(parse_xml(doc).unwrap().as_str(), Some("ok"));
        assert!(parse_xml(b"<!DOCTYPE plist [<!ELEMENT plist ANY><plist><true/></plist>").is_err());
    }

    #[test]
    fn xml_char_refs_require_plain_digits() {
        let text = |s: &str| {
            let doc = format!("<plist version=\"1.0\"><string>{s}</string></plist>");
            parse_xml(doc.as_bytes()).unwrap().as_str().map(str::to_string)
        };
        assert_eq!(text("a&#+65;b&#x+41;").as_deref(), Some("a&#+65;b&#x+41;"));
        assert_eq!(text("&#65;&#x41;").as_deref(), Some("AA"));
        assert_eq!(text("&#;&#x;").as_deref(), Some("&#;&#x;"));
    }

    #[test]
    fn xml_and_binary_agree() {
        let sample = fx::sample_plist();
        let x = parse(fx::to_xml(&sample).as_bytes()).unwrap();
        let b = parse(&fx::to_bplist(&sample)).unwrap();
        assert_eq!(x.get("CFBundleIdentifier").and_then(Value::as_str), Some("com.example.notes"));
        assert_eq!(b.get("CFBundleIdentifier").and_then(Value::as_str), Some("com.example.notes"));
        assert_eq!(b.get("Count").and_then(Value::as_int), Some(-42));
        assert_eq!(x.get("Count").and_then(Value::as_int), Some(-42));
        let j = b.to_json();
        assert_eq!(j["Nested"]["List"][1], "two & <three>");
        assert_eq!(x.to_json()["Nested"]["List"][1], "two & <three>");
        assert_eq!(j["Blob"]["$data"], "AAEC");
        assert_eq!(x.to_json()["Blob"]["$data"], "AAEC");
        assert_eq!(j["Flag"], true);
        assert_eq!(j["Pi"], 3.5);
        assert_eq!(j["When"]["$date"], "2001-01-02T00:00:00Z");
    }

    #[test]
    fn base64_standard_padding_and_unwrapped_binary() {
        for (raw, encoded) in [("", ""), ("f", "Zg=="), ("fo", "Zm8="), ("foo", "Zm9v"), ("foob", "Zm9vYg=="), ("fooba", "Zm9vYmE="), ("foobar", "Zm9vYmFy")] {
            assert_eq!(base64(raw.as_bytes()), encoded);
        }
        assert_eq!(base64(&[0xfb, 0xff, 0xff]), "+///");
        let bytes: Vec<u8> = (0..=255).collect();
        let encoded = base64(&bytes);
        assert_eq!(encoded.len(), 344);
        assert!(!encoded.contains(['\r', '\n']));
        assert_eq!(base64_decode(&encoded).unwrap(), bytes);
        assert!(base64_decode("Zm!9v").is_none());
    }

    #[test]
    fn base64_round_trip() {
        for n in 0..20 {
            let d: Vec<u8> = (0..n).map(|i| (i * 37) as u8).collect();
            assert_eq!(base64_decode(&base64(&d)).unwrap(), d);
        }
        assert_eq!(apple_date_iso(0.0), "2001-01-01T00:00:00Z");
    }

    #[test]
    fn hostile_inputs() {
        let sample = fx::sample_plist();
        let b = fx::to_bplist(&sample);
        let x = fx::to_xml(&sample).into_bytes();
        for m in craft_anything_testkit::mutations(&b).into_iter().chain(craft_anything_testkit::mutations(&x)) {
            let _ = parse(&m);
        }
        for i in 0..b.len() {
            let _ = parse(&b[..i]);
        }
        for seed in 0..200 {
            let mut n = b"bplist00".to_vec();
            n.extend(craft_anything_testkit::noise(seed, 64));
            let _ = parse(&n);
        }
        // A self-referencing array must be refused, not loop.
        let mut cyc = b"bplist00".to_vec();
        cyc.extend_from_slice(&[0xa1, 0x00]); // array of one ref -> object 0
        let table = cyc.len();
        cyc.push(8);
        let mut trailer = vec![0u8; 6];
        trailer.extend_from_slice(&[1, 1]);
        trailer.extend_from_slice(&1u64.to_be_bytes());
        trailer.extend_from_slice(&0u64.to_be_bytes());
        trailer.extend_from_slice(&(table as u64).to_be_bytes());
        cyc.extend(trailer);
        assert!(parse(&cyc).is_err());
        let deep = "<plist>".to_string() + &"<array>".repeat(1000) + &"</array>".repeat(1000) + "</plist>";
        assert!(parse(deep.as_bytes()).is_err());
    }

    #[test]
    fn xml_plist_utf16_byte_orders_parse() {
        // Apple's Core Foundation reader accepts UTF-16 XML plists with either
        // byte-order mark; the reader used to lossy-decode them as UTF-8 and
        // reject the NUL-separated text (issue #333).
        let xml = "<?xml version=\"1.0\" encoding=\"UTF-16\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Message</key><string>hello</string></dict></plist>";
        let units: Vec<u16> = xml.encode_utf16().collect();
        let mut le = vec![0xFF, 0xFE];
        le.extend(units.iter().copied().flat_map(u16::to_le_bytes));
        let mut be = vec![0xFE, 0xFF];
        be.extend(units.iter().copied().flat_map(u16::to_be_bytes));
        for data in [&le[..], &be[..]] {
            let v = parse(data).unwrap_or_else(|e| panic!("UTF-16 XML plist should parse: {e}"));
            assert_eq!(v.get("Message").and_then(Value::as_str), Some("hello"));
        }
    }

    /// A binary plist of `objects` with two-byte offsets and one-byte references; object 0 is the top.
    fn small_bplist(objects: &[Vec<u8>]) -> Vec<u8> {
        let mut b = b"bplist00".to_vec();
        let mut offsets = Vec::new();
        for o in objects {
            offsets.extend_from_slice(&(b.len() as u16).to_be_bytes());
            b.extend_from_slice(o);
        }
        let table = b.len();
        b.extend(offsets);
        b.extend_from_slice(&[0, 0, 0, 0, 0, 0, 2, 1]);
        b.extend_from_slice(&(objects.len() as u64).to_be_bytes());
        b.extend_from_slice(&0u64.to_be_bytes());
        b.extend_from_slice(&(table as u64).to_be_bytes());
        b
    }

    #[test]
    fn binary_plist_does_not_invent_types_or_dictionary_keys() {
        let mut version = small_bplist(&[vec![0x00]]);
        version[7] = b'1';
        assert!(looks_like_plist(&version));
        assert_eq!(parse_binary(&version).unwrap(), Value::Null);
        version[6] = b'1';
        assert!(matches!(parse_binary(&version), Err(FormatError::Unsupported(_))));
        for marker in [0x01, 0x0f, 0x30, 0x32, 0x34] {
            let mut object = vec![marker];
            object.extend_from_slice(&0f64.to_be_bytes());
            assert!(parse_binary(&small_bplist(&[object])).is_err(), "{marker:#x}");
        }
        assert_eq!(parse_binary(&small_bplist(&[vec![0x00]])).unwrap(), Value::Null);
        assert!(parse_binary(&small_bplist(&[vec![0xd1, 1, 2], vec![0x10, 1], vec![0x09]])).is_err());
        assert_eq!(parse_binary(&small_bplist(&[vec![0xd1, 1, 2], vec![0x51, b'1'], vec![0x09]])).unwrap().get("1"), Some(&Value::Bool(true)));
    }

    #[test]
    fn binary_minor_compatibility_keeps_leaf_types_and_budget() {
        for minor in [b'0', b'1', b'9', 0xff] {
            let mut data = small_bplist(&[vec![0x43, 1, 2, 3]]);
            data[7] = minor;
            assert_eq!(parse_binary_within(&data, 3).unwrap(), Value::Data(vec![1, 2, 3]));
            assert!(matches!(parse_binary_within(&data, 2), Err(FormatError::TooLarge(_))));
            let mut uid = small_bplist(&[vec![0x81, 0xab, 0xcd]]);
            uid[7] = minor;
            assert_eq!(parse_binary(&uid).unwrap(), Value::Uid(0xabcd));
        }
    }

    /// An array (or set) of `n` references to object `child`.
    fn repeated(marker: u8, n: u8, child: u8) -> Vec<u8> {
        let mut o = vec![marker, 0x10, n];
        o.extend(std::iter::repeat_n(child, usize::from(n)));
        o
    }

    /// Writers store equal values once: Python's plistlib writes `[0] * 17` as two objects, an
    /// array of 17 references to one integer. Sets share the array branch.
    #[test]
    fn arrays_may_name_one_stored_value_many_times() {
        for marker in [0xaf_u8, 0xcf] {
            let v = parse(&small_bplist(&[repeated(marker, 17, 1), vec![0x10, 0x00]])).unwrap_or_else(|e| panic!("marker {marker:#04x}: {e}"));
            let items = v.as_array().unwrap();
            assert_eq!(items.len(), 17);
            assert!(items.iter().all(|i| i.as_int() == Some(0)));
        }
        // Shared arrays three deep would expand to 255^3 values; the node cap still stops them.
        let nested = small_bplist(&[repeated(0xaf, 255, 1), repeated(0xaf, 255, 2), repeated(0xaf, 255, 3), vec![0x10, 0x00]]);
        assert!(matches!(parse(&nested), Err(FormatError::TooLarge(_))));
    }

    /// One stored leaf of about 1 KiB named 16 x 16 times through two shared arrays. Text is
    /// charged by its decoded size: damaged UTF-8 bytes and lone surrogates become a 3-byte
    /// U+FFFD, and U+0800 takes 3 bytes from 2 stored ones.
    #[test]
    fn repeated_data_and_text_spend_one_byte_budget() {
        let cases: [(u8, u16, Vec<u8>, usize); 5] = [
            (0x4f, 1024, vec![7; 1024], 1024),
            (0x5f, 1024, vec![b'a'; 1024], 1024),
            (0x5f, 1024, vec![0xff; 1024], 3072),
            (0x6f, 512, [0x08, 0x00].repeat(512), 1536),
            (0x6f, 512, [0xd8, 0x00].repeat(512), 1536),
        ];
        for (marker, length, payload, decoded) in cases {
            let mut leaf = vec![marker, 0x11];
            leaf.extend_from_slice(&length.to_be_bytes());
            leaf.extend(payload);
            let b = small_bplist(&[repeated(0xaf, 16, 1), repeated(0xaf, 16, 2), leaf]);
            let v = parse_binary_within(&b, 256 * decoded).unwrap_or_else(|e| panic!("{marker:#04x}, {decoded}: {e}"));
            let size = match &v.as_array().unwrap()[0].as_array().unwrap()[0] {
                Value::Data(d) => d.len(),
                Value::String(s) => s.len(),
                other => panic!("{other:?}"),
            };
            assert_eq!(size, decoded, "{marker:#04x}");
            let r = parse_binary_within(&b, 256 * decoded - 1);
            assert!(matches!(r, Err(FormatError::TooLarge(_))), "{marker:#04x}, {decoded}: {r:?}");
        }
    }

    /// The references name a missing object: the length of the list alone refuses it, before any
    /// child is read or space is reserved for them.
    #[test]
    fn containers_past_the_node_budget_are_refused_before_their_children() {
        for (marker, count) in [(0xaf_u8, MAX_NODES), (0xdf, MAX_NODES / 2)] {
            let mut top = vec![marker, 0x12];
            top.extend_from_slice(&(count as u32).to_be_bytes());
            top.extend(std::iter::repeat_n(9u8, MAX_NODES));
            let r = parse(&small_bplist(&[top]));
            assert!(matches!(r, Err(FormatError::TooLarge(_))), "{marker:#04x}: {r:?}");
        }
    }
}
