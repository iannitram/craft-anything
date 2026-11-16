use craft_anything_js::{
    Limits,
    recover::{RewriteLevel, pretty_print},
};

/// A dynamic with object can supply undefined, including through a nested closure.
#[test]
fn preserves_void_in_dynamic_scopes() {
    for source in
        ["with ({undefined: 42}) { result = void 0; }", "with (scope) { result = () => void 0; }", "with (scope) { with (other) { result = void 0; } }"]
    {
        let (out, _) = pretty_print("app.js", source, RewriteLevel::Standard, &Limits::default()).unwrap();
        assert!(out.contains("void 0"), "{out}");
    }
}

/// Conservatism around with must not disable unrelated boolean rewrites.
#[test]
fn retains_other_safe_rewrites() {
    let (out, _) = pretty_print("app.js", "with (scope) { a = !0; b = void 0; }", RewriteLevel::Standard, &Limits::default()).unwrap();
    assert!(out.contains("true"));
    assert!(out.contains("void 0"));
    let (plain, _) = pretty_print("app.js", "result = void 0;", RewriteLevel::Standard, &Limits::default()).unwrap();
    assert!(plain.contains("undefined"));
}
