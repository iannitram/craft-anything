//! Regression tests for form-encoded and mixed-case percent-escaped secret redaction in HAR inspection.

use craft_anything_analysis::standin::{BuildOptions, build_from_bytes};
use craft_anything_analysis::web_har::{HarOptions, all_strings, contains_ascii_ci, encode_query, inspect_har, replace_needles_with};
use serde_json::json;
use std::collections::BTreeSet;

/// Standard credential headers and exact literal secrets continue to be scrubbed.
#[test]
fn exact_literal_and_auth_header_redaction_works() {
    let har = serde_json::to_vec(&json!({
        "log": {
            "version": "1.2",
            "entries": [{
                "request": {
                    "method": "GET",
                    "url": "https://api.example.test/v1/ping?note=PlainSecret",
                    "headers": [{"name": "Authorization", "value": "Bearer tok"}]
                },
                "response": { "status": 200, "headers": [], "content": { "size": 0, "mimeType": "" } }
            }]
        }
    }))
    .unwrap();
    let out = inspect_har(&har, &HarOptions { hidden_values: vec!["PlainSecret".into()], limit: 10, max_entries: 100, ..Default::default() }).unwrap();
    assert_eq!(out["entries"][0]["url"], "https://api.example.test/v1/ping?note=[redacted]");
    assert_eq!(out["entries"][0]["request_headers"][0]["value"], serde_json::Value::Null);
}

/// `application/x-www-form-urlencoded` (`+` plus `%XX`) and mixed-case `%xx` hex escapes must be redacted everywhere.
#[test]
fn form_urlencoded_and_mixed_case_percent_escaped_secrets_never_leak() {
    // `URLSearchParams({ auth: "Bearer tok==" })` serializes space as `+` and `=` as `%3D`.
    // Many clients also emit lowercase hex escapes (`%2f`, `%3d`) while preserving mixed-case ASCII letters.
    let har = serde_json::to_vec(&json!({
        "log": {
            "version": "1.2",
            "entries": [{
                "request": {
                    "method": "POST",
                    "url": "https://api.example.test/v1/callback?state=Bearer+tok%3D%3D&next=MySecret%2f1&upper=MYSECRET/1",
                    "headers": [
                        {"name": "X-Custom-Trace", "value": "hint=Bearer+tok%3d%3d; path=MySecret%2f1"},
                        {"name": "Referer", "value": "https://app.example.test/cb?s=MySecret%2f1"}
                    ],
                    "postData": {
                        "mimeType": "application/x-www-form-urlencoded",
                        "text": "safe_field=1&MySecret%2f1=val&Bearer+tok%3D%3D=2"
                    }
                },
                "response": {
                    "status": 200,
                    "headers": [],
                    "content": {
                        "size": 32,
                        "mimeType": "application/json",
                        "text": "{\"ok\":true,\"MYSECRET/1\":\"v\"}"
                    }
                }
            }]
        }
    }))
    .unwrap();

    let out =
        inspect_har(&har, &HarOptions { hidden_values: vec!["Bearer tok==".into(), "MySecret/1".into()], limit: 10, max_entries: 100, ..Default::default() })
            .unwrap();

    let mut strings = BTreeSet::new();
    all_strings(&out, &mut strings);
    for s in &strings {
        let low = s.to_ascii_lowercase();
        for forbidden in ["tok==", "tok%3d%3d", "mysecret/1", "mysecret%2f1"] {
            assert!(!low.contains(forbidden), "secret spelling `{forbidden}` leaked in `{s}`");
        }
    }

    assert_eq!(out["entries"][0]["request_body"]["fields"], json!(["safe_field"]));
    assert!(
        out["redactions"].as_array().unwrap().iter().any(|r| r["scope"] == "form-field"),
        "dropping secret form field names should record a redaction entry: {}",
        out["redactions"]
    );
}

/// Multipart `params` field names carrying percent-encoded or uppercase spellings of declared secrets are dropped and logged.
#[test]
fn multipart_encoded_secret_field_names_are_filtered_and_logged() {
    let har = serde_json::to_vec(&json!({
        "log": {
            "version": "1.2",
            "entries": [{
                "request": {
                    "method": "POST",
                    "url": "https://api.example.test/v1/upload",
                    "headers": [],
                    "postData": {
                        "mimeType": "multipart/form-data; boundary=---x",
                        "params": [
                            {"name": "avatar", "value": "file"},
                            {"name": "Key%20One", "value": "x"},
                            {"name": "KEY ONE", "value": "y"}
                        ]
                    }
                },
                "response": { "status": 200, "headers": [], "content": { "size": 0, "mimeType": "" } }
            }]
        }
    }))
    .unwrap();

    let out = inspect_har(&har, &HarOptions { hidden_values: vec!["Key One".into()], limit: 10, max_entries: 100, ..Default::default() }).unwrap();
    assert_eq!(out["entries"][0]["request_body"]["fields"], json!(["avatar"]));
    assert!(out["redactions"].as_array().unwrap().iter().any(|r| r["scope"] == "form-field"));
}

/// `standin::build_from_bytes` scrubs form-encoded (`+` with `%XX`) and mixed-case percent-escaped secrets from URLs, headers, and bodies.
#[test]
fn standin_build_redacts_form_urlencoded_and_mixed_case_percent_escaped_secrets() {
    let har = serde_json::to_vec(&json!({
        "log": {
            "version": "1.2",
            "entries": [
                {
                    "request": {
                        "method": "POST",
                        "url": "https://api.example.test/v1/MYSECRET%2f1/items?Bearer+Tok%3d%3D=1&ok=MySecret%2f1&Key%20One=2",
                        "headers": []
                    },
                    "response": {
                        "status": 200,
                        "headers": [
                            {"name": "X-Custom-Trace", "value": "hint=Bearer+Tok%3d%3d; next=MYSECRET%2f1"}
                        ],
                        "content": {
                            "size": 64,
                            "mimeType": "application/json",
                            "text": "{\"ok\":true,\"MYSECRET%2f1\":\"Bearer+Tok%3d%3D\"}"
                        }
                    }
                },
                {
                    "request": {
                        "method": "GET",
                        "url": "https://api.example.test/v1/echo",
                        "headers": []
                    },
                    "response": {
                        "status": 200,
                        "headers": [],
                        "content": {
                            "size": 64,
                            "mimeType": "text/plain",
                            "text": "token=Bearer+Tok%3d%3d secret=MYSECRET%2f1"
                        }
                    }
                }
            ]
        }
    }))
    .unwrap();

    let built =
        build_from_bytes(&har, &BuildOptions { hidden_values: vec!["Bearer tok==".into(), "MySecret/1".into(), "Key One".into()], ..Default::default() })
            .unwrap();
    assert!(built.dropped.hidden_values > 0);
    let serialized = serde_json::to_value(&built).unwrap();
    let mut strings = BTreeSet::new();
    all_strings(&serialized, &mut strings);
    for s in &strings {
        let low = s.to_ascii_lowercase();
        for forbidden in ["tok==", "tok%3d%3d", "mysecret/1", "mysecret%2f1", "key one", "key%20one"] {
            assert!(!low.contains(forbidden), "stand-in leaked secret spelling `{forbidden}` in `{s}`");
        }
    }
}

/// WHATWG `application/x-www-form-urlencoded` keeps `*` literal and encodes `~` as `%7E`,
/// while Python `urllib.parse.quote_plus` keeps `~` literal and encodes `*` as `%2A`; both must redact.
#[test]
fn form_urlencoded_asterisk_and_tilde_redact_in_har_and_standin() {
    let har = serde_json::to_vec(&json!({
        "log": {
            "version": "1.2",
            "entries": [{
                "request": {
                    "method": "POST",
                    "url": "https://api.example.test/v1/a*b%7E+c/call?form=a*b%7E+c&py=a%2Ab~+c&gen=a%2Ab~%20c&a*b%7e+c=1",
                    "headers": [
                        {"name": "X-Trace", "value": "v=A*B%7e+C; py=a%2Ab~+c"}
                    ],
                    "postData": {
                        "mimeType": "application/x-www-form-urlencoded",
                        "text": "a*b%7E+c=ok&a%2Ab~+c=ok&safe=1"
                    }
                },
                "response": {
                    "status": 200,
                    "headers": [
                        {"name": "X-Reply", "value": "echo=a*b%7E+c; py=a%2ab~+c"}
                    ],
                    "content": {
                        "size": 48,
                        "mimeType": "application/json",
                        "text": "{\"val\":\"a*b%7e+c\",\"py\":\"a%2Ab~+c\",\"gen\":\"a%2ab~%20c\"}"
                    }
                }
            }]
        }
    }))
    .unwrap();

    let hidden = vec!["a*b~ c".into()];
    let har_out = inspect_har(&har, &HarOptions { hidden_values: hidden.clone(), limit: 10, max_entries: 100, ..Default::default() }).unwrap();
    let mut har_strings = BTreeSet::new();
    all_strings(&har_out, &mut har_strings);
    for s in &har_strings {
        let low = s.to_ascii_lowercase();
        for forbidden in ["a*b~ c", "a*b%7e+c", "a%2ab~+c", "a%2ab~%20c"] {
            assert!(!low.contains(forbidden), "HAR leaked `{forbidden}` in `{s}`");
        }
    }
    assert_eq!(har_out["entries"][0]["request_body"]["fields"], json!(["safe"]));

    let standin = build_from_bytes(&har, &BuildOptions { hidden_values: hidden, ..Default::default() }).unwrap();
    let serialized = serde_json::to_value(&standin).unwrap();
    let mut standin_strings = BTreeSet::new();
    all_strings(&serialized, &mut standin_strings);
    for s in &standin_strings {
        let low = s.to_ascii_lowercase();
        for forbidden in ["a*b~ c", "a*b%7e+c", "a%2ab~+c", "a%2ab~%20c"] {
            assert!(!low.contains(forbidden), "stand-in leaked `{forbidden}` in `{s}`");
        }
    }
}

/// Overlapping, adjacent, and same-origin declared secrets must merge cleanly in HAR inspection and stand-in builds.
#[test]
fn overlapping_and_adjacent_secrets_never_leak_suffixes_in_har_or_standin() {
    let har = serde_json::to_vec(&json!({
        "log": {
            "version": "1.2",
            "entries": [{
                "request": {
                    "method": "GET",
                    "url": "https://api.example.test/v1/echo?id=alpha-beta-gamma&q=key+one%2ftwo&adj=pfx-tok999&same=tok-extended&iso=tok",
                    "headers": [
                        {"name": "X-Trace", "value": "Bearer abc123456789"},
                        {"name": "X-Prefix", "value": "Prefix abc123456"},
                        {"name": "X-Utf8", "value": "前Prefix abc123456后"}
                    ]
                },
                "response": {
                    "status": 200,
                    "headers": [
                        {"name": "X-Echo", "value": "id=alpha-beta-gamma; q=key+one%2ftwo; adj=pfx-tok999"}
                    ],
                    "content": {
                        "size": 120,
                        "mimeType": "text/plain",
                        "text": "Bearer abc123456789 | Prefix abc123456 | 前Prefix abc123456后 | alpha-beta-gamma | key+one%2ftwo | pfx-tok999 | tok-extended | tok"
                    }
                }
            }]
        }
    }))
    .unwrap();

    let hidden = vec![
        "Bearer abc".into(),
        "abc123456789".into(),
        "Prefix abc".into(),
        "abc123456".into(),
        "alpha-beta".into(),
        "beta-gamma".into(),
        "Key One".into(),
        "One/Two".into(),
        "pfx-".into(),
        "tok999".into(),
        "tok".into(),
        "tok-extended".into(),
    ];

    let har_out = inspect_har(&har, &HarOptions { hidden_values: hidden.clone(), limit: 10, max_entries: 100, ..Default::default() }).unwrap();
    assert_eq!(har_out["entries"][0]["request_headers"][0]["value"], "[redacted]");
    assert_eq!(har_out["entries"][0]["request_headers"][1]["value"], "[redacted]");
    assert_eq!(har_out["entries"][0]["request_headers"][2]["value"], "前[redacted]后");
    let mut har_strings = BTreeSet::new();
    all_strings(&har_out, &mut har_strings);
    for s in &har_strings {
        let low = s.to_ascii_lowercase();
        for forbidden in ["123456789", "123456", "gamma", "two", "tok999", "extended"] {
            assert!(!low.contains(forbidden), "HAR leaked `{forbidden}` in `{s}`");
        }
    }

    let standin = build_from_bytes(&har, &BuildOptions { hidden_values: hidden, ..Default::default() }).unwrap();
    let body = standin.routes[0].samples[0].text.as_deref().unwrap();
    // `standin` scrubs `Bearer <credential>` tokens before `hide`, then `hide` merges overlapping spans (`Prefix abc` + `abc123456`, etc.) and replaces non-overlapping adjacent spans individually.
    assert_eq!(body, "Bearer [dropped] | [dropped] | 前[dropped]后 | [dropped] | [dropped] | [dropped][dropped] | [dropped] | [dropped]");
    let serialized = serde_json::to_value(&standin).unwrap();
    let mut standin_strings = BTreeSet::new();
    all_strings(&serialized, &mut standin_strings);
    for s in &standin_strings {
        let low = s.to_ascii_lowercase();
        for forbidden in ["123456789", "123456", "gamma", "two", "tok999", "extended"] {
            assert!(!low.contains(forbidden), "stand-in leaked `{forbidden}` in `{s}`");
        }
    }

    // Direct checks on the shared public helpers, including unsorted needles.
    assert!(contains_ascii_ci("Auth=Bearer+Tok%3d%3d", "bearer+tok%3D%3D"));
    assert_eq!(encode_query("a*b~ c", true, true), "a*b%7E+c");
    assert_eq!(encode_query("a*b~ c", false, false), "a%2ab~%20c");
    let unsorted = vec!["tok".to_string(), "tok-extended".to_string(), "Bearer abc".to_string(), "abc123456789".to_string()];
    assert_eq!(replace_needles_with("Bearer abc123456789 & tok-extended & tok", &unsorted, "«redacted»"), ("«redacted» & «redacted» & «redacted»".into(), 3));
}
