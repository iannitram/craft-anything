//! Text read through constants joined both sides of every `+` as written in the source, which
//! is not JavaScript's `+`: `'/v' + (1 + 2)` read as `/v12` and `'/w' + 0x10` as `/w0x10`, both
//! marked literal. Two numbers add, and a number reads as JavaScript turns it into text.

use craft_anything_js::{Analysis, Limits, ParseMode, analyze};

fn parsed(src: &str) -> Analysis {
    let analysis = analyze("app.js", src, &Limits::default());
    assert_eq!(analysis.facts.mode, ParseMode::Parsed, "{:?}", analysis.facts.mode_reason);
    analysis
}

/// The URL of the single `fetch` in `src` and whether it is literal.
fn fetched(src: &str) -> (String, bool) {
    let a = parsed(src);
    let fetches: Vec<_> = a.facts.endpoints.iter().filter(|e| e.api == "fetch").collect();
    assert_eq!(fetches.len(), 1, "{src}: {:?}", a.facts.endpoints);
    (fetches[0].url.clone(), fetches[0].literal)
}

#[test]
fn numbers_add() {
    for (src, url) in [
        ("fetch('/v' + (1 + 2));", "/v3"),
        ("fetch(1 + 2 + '/y');", "3/y"),
        ("fetch(`/v${1 + 2}`);", "/v3"),
        ("const n = 1;\nfetch('/v' + (n + 1));", "/v2"),
        ("const a = 1, b = a + 1;\nfetch('/v' + b);", "/v2"),
    ] {
        assert_eq!(fetched(src), (url.to_string(), true), "{src}");
    }
}

#[test]
fn numbers_read_as_javascript_writes_them() {
    for (src, url) in [
        ("fetch('/w' + 0x10);", "/w16"),
        ("fetch('/e' + 1e3);", "/e1000"),
        ("fetch('/s' + 1_000);", "/s1000"),
        ("fetch('/f' + 1.50);", "/f1.5"),
        ("fetch('/o' + 0o17 + '/' + 0b11);", "/o15/3"),
    ] {
        assert_eq!(fetched(src), (url.to_string(), true), "{src}");
    }
}

/// Text on either side still joins, left to right.
#[test]
fn text_still_joins() {
    for (src, url) in [
        ("fetch('/x' + 1 + 2);", "/x12"),
        ("const port = 8080;\nfetch('http://localhost:' + port + '/api');", "http://localhost:8080/api"),
        ("const s = '1';\nfetch('/t' + (s + 2));", "/t12"),
        ("const base = '/api';\nfetch(base + '/v' + 2);", "/api/v2"),
    ] {
        assert_eq!(fetched(src), (url.to_string(), true), "{src}");
    }
}

/// A number added to something unreadable may be a number or text, so it is not literal.
#[test]
fn unknown_operands_are_not_literal() {
    let (url, literal) = fetched("fetch('/u' + (x + 1));");
    assert!(!literal, "{url}");
    let (url, literal) = fetched("const n = 1;\nfetch('/u' + (n + x));");
    assert!(!literal, "{url}");
}
