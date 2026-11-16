//! Facts read text through constants (`const url = base + "/v1"; fetch(url)`). Concatenating a
//! constant with itself doubles that text with every declaration, so a few hundred bytes of source
//! could ask for gigabytes. Built text is bounded and stays readable at ordinary sizes.

use craft_anything_js::{Analysis, Limits, ParseMode, analyze};

/// Analyse `src` and check the syntax tree was used.
fn parsed(src: &str) -> Analysis {
    let analysis = analyze("app.js", src, &Limits::default());
    assert_eq!(analysis.facts.mode, ParseMode::Parsed, "{:?}", analysis.facts.mode_reason);
    analysis
}

/// `const a0 = 'xxxxxxxx'; const a1 = a0 + a0; …` with `doublings` declarations after the first,
/// then a fetch of the last one.
fn doubling(doublings: usize) -> String {
    let mut src = String::from("const a0 = 'xxxxxxxx';\n");
    for i in 1..=doublings {
        src.push_str(&format!("const a{i} = a{p} + a{p};\n", p = i - 1));
    }
    src.push_str(&format!("fetch(a{doublings});\n"));
    src
}

/// Twenty-six doublings asked for 512 MiB in one string (1.5 GB at peak); the result is now
/// reported as not literal instead.
#[test]
fn self_concatenation_stops_at_the_budget() {
    let analysis = parsed(&doubling(26));
    let endpoint = analysis.facts.endpoints.first().expect("the fetch is still reported");
    assert!(!endpoint.literal);
    assert!(endpoint.url.len() <= 4096, "{} bytes", endpoint.url.len());
}

/// Sixty-four doublings would need exabytes; the analysis finishes.
#[test]
fn far_more_doublings_still_finish() {
    let analysis = parsed(&doubling(64));
    assert!(analysis.facts.endpoints.iter().all(|endpoint| endpoint.url.len() <= 4096));
}

/// Many separate declarations of one long value share one budget for all constants.
#[test]
fn every_constant_shares_one_budget() {
    let mut src = format!("const a = '{}';\n", "x".repeat(1_000));
    for i in 0..2_000 {
        src.push_str(&format!("const b{i} = a + a + a + a;\n"));
    }
    src.push_str("fetch(b0);\nfetch(b1999);\n");
    let analysis = parsed(&src);
    let urls: Vec<_> =
        analysis.facts.endpoints.iter().filter(|endpoint| endpoint.api == "fetch").map(|endpoint| (endpoint.url.len(), endpoint.literal)).collect();
    assert_eq!(urls.first(), Some(&(4_000, true)), "an early constant is still read");
    assert_eq!(urls.get(1).map(|url| url.1), Some(false), "the budget is spent by then: {urls:?}");
}

/// A later declaration of the same name replaces the earlier value even when the new one is too
/// long, or isn't a constant at all: the old URL must not be reported as what is fetched.
#[test]
fn a_new_value_replaces_the_old_one() {
    let long = format!("var u = 'https://first.example/'; var u = 'https://second.example/' + '{}'; fetch(u);", "a".repeat(5_000));
    let unknown = "var u = 'https://first.example/'; var u = load(); fetch(u);";
    let own = "var u = 'https://first.example'; var u = u + '/v1'; fetch(u);";
    for src in [long.as_str(), unknown] {
        let analysis = parsed(src);
        let fetched: Vec<_> = analysis.facts.endpoints.iter().filter(|endpoint| endpoint.api == "fetch").collect();
        assert!(fetched.iter().all(|endpoint| !endpoint.literal && !endpoint.url.contains("first")), "{fetched:?}");
    }
    let analysis = parsed(own);
    let fetched: Vec<_> =
        analysis.facts.endpoints.iter().filter(|endpoint| endpoint.api == "fetch").map(|endpoint| (endpoint.url.as_str(), endpoint.literal)).collect();
    assert_eq!(fetched, [("https://first.example/v1", true)]);
}

/// The new declaration's initializer still reads the old value: `fetch(u)` here fetches the first
/// URL. Only after it is the value replaced.
#[test]
fn the_initializer_still_reads_the_old_value() {
    let analysis = parsed("var u = 'https://first.example/'; var u = fetch(u); fetch(u);");
    let fetched: Vec<_> =
        analysis.facts.endpoints.iter().filter(|endpoint| endpoint.api == "fetch").map(|endpoint| (endpoint.url.as_str(), endpoint.literal)).collect();
    assert_eq!(fetched, [("https://first.example/", true), ("{u}", false)]);
}

/// Ordinary constants keep resolving through concatenation and templates.
#[test]
fn ordinary_constants_still_resolve() {
    let analysis =
        parsed("const base = 'https://api.example.com';\nconst users = base + '/v1/' + 'users';\nconst one = `${users}/42`;\nfetch(users);\nfetch(one);\n");
    let urls: Vec<_> =
        analysis.facts.endpoints.iter().filter(|endpoint| endpoint.api == "fetch").map(|endpoint| (endpoint.url.as_str(), endpoint.literal)).collect();
    assert_eq!(urls, [("https://api.example.com/v1/users", true), ("https://api.example.com/v1/users/42", true)]);
}
