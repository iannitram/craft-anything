//! Regression tests for `NSString` (`NS.bytes`) and `NSDate` (`NS.time`) projection in `keyed_archive`.

use craft_anything_formats::keyed_archive::{KeyedArchive, from_plist, parse};
use craft_anything_formats::plist::Value;
use craft_anything_testkit::apple as fx;
use serde_json::json;

/// Build a `$class` dictionary entry in `$objects`.
fn class_dict(name: &str) -> Value {
    Value::Dict(vec![("$classname".into(), Value::String(name.into()))])
}

/// Inline `NS.string` and `Value::Real` `NS.time` continue to project as expected.
#[test]
fn inline_nsstring_and_real_nsdate_continue_to_project() {
    let archive = fx::keyed_archive();
    let parsed = parse(&archive).unwrap();
    let (_, root) = parsed.root(None).unwrap();
    assert_eq!(parsed.project(root)["title"], "Groceries");
}

/// `NSString` / `NSMutableString` whose `NS.bytes` is a UID or `CF$UID` reference into `$objects` projects as a string.
#[test]
fn indirect_uid_nsbytes_projects_to_utf8_string_in_binary_and_xml() {
    let plist_val = Value::Dict(vec![
        ("$archiver".into(), Value::String("NSKeyedArchiver".into())),
        ("$version".into(), Value::Integer(100_000)),
        ("$top".into(), Value::Dict(vec![("root".into(), Value::Uid(1))])),
        (
            "$objects".into(),
            Value::Array(vec![
                Value::String("$null".into()),
                Value::Dict(vec![("$class".into(), Value::Uid(2)), ("greeting".into(), Value::Uid(3)), ("mutable".into(), Value::Uid(5))]),
                class_dict("Envelope"),
                Value::Dict(vec![("$class".into(), Value::Uid(4)), ("NS.bytes".into(), Value::Uid(6))]),
                class_dict("NSString"),
                Value::Dict(vec![("$class".into(), Value::Uid(7)), ("NS.bytes".into(), Value::Uid(6))]),
                Value::Data("héllo 🌍".as_bytes().to_vec()),
                class_dict("NSMutableString"),
            ]),
        ),
    ]);

    let archive = from_plist(&plist_val).unwrap();
    let projected = archive.project(1);
    assert_eq!(projected["greeting"], json!("héllo 🌍"), "{projected}");
    assert_eq!(projected["mutable"], json!("héllo 🌍"), "{projected}");

    // XML keyed-archive form uses `{"CF$UID": n}` dictionaries instead of binary UIDs.
    let xml = fx::to_xml(&fx::dict(vec![
        ("$archiver", fx::s("NSKeyedArchiver")),
        ("$version", fx::P::Int(100_000)),
        ("$top", fx::dict(vec![("root", fx::P::Uid(1))])),
        (
            "$objects",
            fx::P::Array(vec![
                fx::s("$null"),
                fx::dict(vec![("$class", fx::P::Uid(2)), ("greeting", fx::P::Uid(3)), ("mutable", fx::P::Uid(5))]),
                fx::dict(vec![("$classname", fx::s("Envelope"))]),
                fx::dict(vec![("$class", fx::P::Uid(4)), ("NS.bytes", fx::P::Uid(6))]),
                fx::dict(vec![("$classname", fx::s("NSString"))]),
                fx::dict(vec![("$class", fx::P::Uid(7)), ("NS.bytes", fx::P::Uid(6))]),
                fx::P::Data("héllo 🌍".as_bytes().to_vec()),
                fx::dict(vec![("$classname", fx::s("NSMutableString"))]),
            ]),
        ),
    ]));
    let xml_archive = parse(xml.as_bytes()).unwrap();
    let xml_projected = xml_archive.project(1);
    assert_eq!(xml_projected["greeting"], json!("héllo 🌍"), "{xml_projected}");
    assert_eq!(xml_projected["mutable"], json!("héllo 🌍"), "{xml_projected}");
}

/// `NSDate` with integer `NS.time` or UID-referenced `NS.time` projects to `{"$date": ...}`.
#[test]
fn integer_and_uid_referenced_nstime_project_to_iso_dates() {
    let archive = KeyedArchive {
        archiver: "NSKeyedArchiver".into(),
        version: Some(100_000),
        top: vec![("root".into(), 1)],
        objects: vec![
            Value::String("$null".into()),
            Value::Dict(vec![
                ("$class".into(), Value::Uid(2)),
                ("epoch_int".into(), Value::Uid(3)),
                ("uid_real".into(), Value::Uid(5)),
                ("uid_int".into(), Value::Uid(7)),
            ]),
            class_dict("Timestamps"),
            Value::Dict(vec![("$class".into(), Value::Uid(4)), ("NS.time".into(), Value::Integer(0))]),
            class_dict("NSDate"),
            Value::Dict(vec![("$class".into(), Value::Uid(4)), ("NS.time".into(), Value::Uid(6))]),
            Value::Real(86_400.5),
            Value::Dict(vec![("$class".into(), Value::Uid(4)), ("NS.time".into(), Value::Uid(8))]),
            Value::Integer(86_400),
        ],
    };

    let projected = archive.project(1);
    assert_eq!(projected["epoch_int"], json!({"$date": "2001-01-01T00:00:00Z"}), "{projected}");
    assert_eq!(projected["uid_real"], json!({"$date": "2001-01-02T00:00:00.5Z"}), "{projected}");
    assert_eq!(projected["uid_int"], json!({"$date": "2001-01-02T00:00:00Z"}), "{projected}");
}

/// Repeated `NS.bytes` expansions respect the shared `MAX_REFERENCED_BYTES` projection budget.
#[test]
fn repeated_nsbytes_expansions_are_bounded_by_projection_byte_budget() {
    // 1 MiB payload referenced by 80 distinct `NSString` wrappers (80 MiB > 64 MiB budget).
    let chunk = vec![b'x'; 1024 * 1024];
    let mut objects = vec![Value::String("$null".into()), class_dict("NSArray"), class_dict("NSString"), Value::Data(chunk)];
    let mut item_uids = Vec::new();
    for i in 0..80u64 {
        let uid = objects.len() as u64;
        objects.push(Value::Dict(vec![("$class".into(), Value::Uid(2)), ("NS.bytes".into(), Value::Uid(3))]));
        let _ = i;
        item_uids.push(Value::Uid(uid));
    }
    let root_uid = objects.len() as u64;
    objects.push(Value::Dict(vec![("$class".into(), Value::Uid(1)), ("NS.objects".into(), Value::Array(item_uids))]));

    let archive = KeyedArchive { archiver: "NSKeyedArchiver".into(), version: Some(100_000), top: vec![("root".into(), root_uid)], objects };
    let projected = archive.project(root_uid);
    let items = projected["items"].as_array().unwrap();
    assert_eq!(items.len(), 80);
    assert!(items[0].is_string());
    assert!(items.iter().any(|it| it["$note"] == "projection byte limit"), "expected byte budget cap to stop unbounded NS.bytes expansion");
}

/// Integer `NS.time` values beyond `plist`'s `1e13` second limit fall back to exact decimal strings.
#[test]
fn out_of_range_integer_nstime_falls_back_to_exact_integer_string() {
    let archive = KeyedArchive {
        archiver: "NSKeyedArchiver".into(),
        version: Some(100_000),
        top: vec![("root".into(), 1)],
        objects: vec![
            Value::String("$null".into()),
            Value::Dict(vec![
                ("$class".into(), Value::Uid(2)),
                ("at_limit".into(), Value::Uid(3)),
                ("over_limit".into(), Value::Uid(5)),
                ("under_neg_limit".into(), Value::Uid(6)),
                ("beyond_f64_exact".into(), Value::Uid(7)),
            ]),
            class_dict("BoundaryDates"),
            Value::Dict(vec![("$class".into(), Value::Uid(4)), ("NS.time".into(), Value::Integer(10_000_000_000_000))]),
            class_dict("NSDate"),
            Value::Dict(vec![("$class".into(), Value::Uid(4)), ("NS.time".into(), Value::Integer(10_000_000_000_001))]),
            Value::Dict(vec![("$class".into(), Value::Uid(4)), ("NS.time".into(), Value::Integer(-10_000_000_000_001))]),
            Value::Dict(vec![("$class".into(), Value::Uid(4)), ("NS.time".into(), Value::Integer(100_000_000_000_000_001))]),
        ],
    };

    let projected = archive.project(1);
    assert_eq!(projected["at_limit"], json!({"$date": "318888-05-20T17:46:40Z"}), "{projected}");
    assert_eq!(projected["at_limit"], json!({"$date": craft_anything_formats::plist::apple_date_iso(10_000_000_000_000.0)}), "{projected}");
    assert_eq!(projected["over_limit"], json!({"$date": "10000000000001"}), "{projected}");
    assert_eq!(projected["under_neg_limit"], json!({"$date": "-10000000000001"}), "{projected}");
    assert_eq!(projected["beyond_f64_exact"], json!({"$date": "100000000000000001"}), "{projected}");
}

/// `resolve_scalar` follows exactly 16 UID hops for `NS.bytes` and `NS.time` and refuses 17 hops.
#[test]
fn resolve_scalar_accepts_sixteen_uid_hops_and_rejects_seventeen() {
    // Build a 17-hop UID chain ending in a scalar at the tail, then point roots at hop-count 16 and 17.
    let mut objects = vec![
        Value::String("$null".into()),
        class_dict("HopBox"),
        class_dict("NSString"),
        class_dict("NSDate"),
        Value::Data(b"ok16".to_vec()),
        Value::Integer(0),
    ];
    // For NS.bytes: 17 hops means: wrapper -> uid_1 -> uid_2 -> ... -> uid_16 -> uid_17 -> Data.
    // Let's build an explicit 17-entry chain of Value::Uid nodes pointing down to `data_idx`.
    let data_idx = 4u64;
    let time_idx = 5u64;
    let mut make_chain = |leaf: u64, hops: usize| -> u64 {
        let mut cur = leaf;
        for _ in 0..hops {
            let idx = objects.len() as u64;
            objects.push(Value::Uid(cur));
            cur = idx;
        }
        cur
    };
    // Note: `wrapper["NS.bytes"]` itself holds the 1st Uid (`hops_remaining = 16` after reading `wrapper["NS.bytes"]` = 1st hop).
    // Putting `Value::Uid(head_of_15)` in `NS.bytes` takes 1 + 15 = 16 UID hops total to reach the leaf.
    // Putting `Value::Uid(head_of_16)` in `NS.bytes` takes 1 + 16 = 17 UID hops total.
    let bytes_15 = make_chain(data_idx, 15);
    let bytes_16 = make_chain(data_idx, 16);
    let time_15 = make_chain(time_idx, 15);
    let time_16 = make_chain(time_idx, 16);

    let s16 = objects.len() as u64;
    objects.push(Value::Dict(vec![("$class".into(), Value::Uid(2)), ("NS.bytes".into(), Value::Uid(bytes_15))]));
    let s17 = objects.len() as u64;
    objects.push(Value::Dict(vec![("$class".into(), Value::Uid(2)), ("NS.bytes".into(), Value::Uid(bytes_16))]));
    let d16 = objects.len() as u64;
    objects.push(Value::Dict(vec![("$class".into(), Value::Uid(3)), ("NS.time".into(), Value::Uid(time_15))]));
    let d17 = objects.len() as u64;
    objects.push(Value::Dict(vec![("$class".into(), Value::Uid(3)), ("NS.time".into(), Value::Uid(time_16))]));

    let root = objects.len() as u64;
    objects.push(Value::Dict(vec![
        ("$class".into(), Value::Uid(1)),
        ("str16".into(), Value::Uid(s16)),
        ("str17".into(), Value::Uid(s17)),
        ("date16".into(), Value::Uid(d16)),
        ("date17".into(), Value::Uid(d17)),
    ]));

    let archive = KeyedArchive { archiver: "NSKeyedArchiver".into(), version: Some(100_000), top: vec![("root".into(), root)], objects };
    let projected = archive.project(root);
    assert_eq!(projected["str16"], json!("ok16"), "16 UID hops must resolve NS.bytes: {projected}");
    assert_eq!(projected["date16"], json!({"$date": "2001-01-01T00:00:00Z"}), "16 UID hops must resolve NS.time: {projected}");
    assert_eq!(projected["str17"]["$class"], json!("NSString"), "17 UID hops must remain unprojected: {projected}");
    assert_eq!(projected["date17"]["$class"], json!("NSDate"), "17 UID hops must remain unprojected: {projected}");
}
