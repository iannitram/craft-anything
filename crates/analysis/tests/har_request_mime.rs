use craft_anything_analysis::web_har::har_from_network;
use craft_anything_formats::har::parse;
use serde_json::{Value, json};

/// Round-trip one captured request through the HAR writer and reader.
fn archived(headers: Value) -> craft_anything_formats::har::HarEntry {
    let requests = [json!({
        "method": "POST",
        "url": "https://example.test/form",
        "status": 200,
        "mime_type": "application/json",
        "request_headers": headers,
        "request_body": {"state": "captured", "text": "name=example"},
        "response_body": {"state": "captured", "text": "{}"}
    })];
    let output = har_from_network(&requests, "2026-01-01T00:00:00Z");
    parse(&serde_json::to_vec(&output).unwrap(), 1).unwrap().entries.remove(0)
}

/// The request MIME comes from its own headers, including outlined header maps.
#[test]
fn preserves_the_request_content_type_separately_from_the_response() {
    for headers in [json!({"Content-TYpe": "application/x-www-form-urlencoded"}), json!({"kept": {"content-type": "application/x-www-form-urlencoded"}})] {
        let entry = archived(headers);
        let post = entry.request.post.unwrap();
        assert_eq!(post.mime.as_deref(), Some("application/x-www-form-urlencoded"));
        assert_eq!(post.text.as_deref(), Some("name=example"));
        assert_eq!(entry.response.mime.as_deref(), Some("application/json"));
    }
}

/// Missing or redacted request header values stay unknown.
#[test]
fn leaves_the_post_type_empty_when_the_request_header_value_is_unavailable() {
    for headers in [json!({}), json!({"content-type": 16})] {
        assert_eq!(archived(headers).request.post.unwrap().mime.as_deref(), Some(""));
    }
}
