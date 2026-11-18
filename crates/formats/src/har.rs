//! Shared HAR records and parser; input admission, redaction and analysis stay app-owned.
pub use craft_har::{Har, HarBody, HarEntry, HarError, HarRequest, HarResponse, parse};

#[cfg(test)]
#[path = "har_legacy.rs"]
mod legacy;

#[cfg(test)]
mod parity_tests {
    use super::*;
    #[test]
    fn retained_records_errors_and_truncation_match() {
        for data in [b"".as_slice(), b"{}", b"bad", br#"{"log":{"entries":[]}}"#, br#"{"log":{"entries":[null,{},1,{"request":{"method":"GET","url":"https://example.test","headers":[{"name":"X-Test","value":"v"}]},"response":{"status":200,"content":{"text":"body"}}}]}}"#] {
            for limit in [0, 1, 2, 20] {
                assert_eq!(format!("{:?}", legacy::parse(data, limit)), format!("{:?}", parse(data, limit)));
            }
        }
    }
    #[test]
    fn bom_and_populated_records_match_at_each_limit() {
        let capture = br#"{"log":{"entries":[{"startedDateTime":"2026-02-03T04:05:06Z","time":4.75,"request":{"method":"PUT","url":"https://capture.example.test/items","cookies":[{"name":"session","value":"fixture"}],"postData":{"mimeType":"text/plain","text":"payload"}},"response":{"status":202,"cookies":[{"name":"result","value":"fixture"}],"content":{"mimeType":"text/plain","text":"received"}},"timings":{"wait":3.5,"receive":1.25}}]}}"#;
        let mut bom = vec![0xef, 0xbb, 0xbf];
        bom.extend_from_slice(capture);
        for data in [capture.as_slice(), bom.as_slice(), &[0xef, 0xbb]] {
            for limit in [0, 1, 2, 20] {
                assert_eq!(format!("{:?}", legacy::parse(data, limit)), format!("{:?}", parse(data, limit)));
            }
        }
    }
}
