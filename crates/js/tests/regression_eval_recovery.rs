use craft_anything_js::recover::{RecoveredModule, RewriteLevel, recover_modules};
use craft_anything_js::{Limits, ParseMode, analyze};

/// Recover the target factory alongside an ordinary module as a control.
fn recover(body: &str) -> Vec<RecoveredModule> {
    let source = format!("var __webpack_modules__ = {{1: function(e,t,n) {{{body}}}, 2: function(e,t,n) {{e.exports=2;}}}};");
    let limits = Limits::default();
    let analysis = analyze("bundle.js", &source, &limits);
    assert_eq!(analysis.facts.mode, ParseMode::Parsed);
    assert_eq!(analysis.facts.parse_errors, 0);
    let modules = recover_modules(&source, &analysis.bundle.modules, RewriteLevel::Standard, &limits);
    assert_eq!(modules.len(), 2);
    assert_eq!(modules.iter().find(|m| m.id == "2").unwrap().renamed.len(), 3);
    modules
}

/// Eval may read every parameter through text unavailable to reference analysis.
#[test]
fn direct_eval_keeps_factory_bindings() {
    for body in [r#"e.exports = eval("e === t && typeof n === 'function'");"#, r#"e.exports = eval(t.source);"#] {
        let modules = recover(body);
        let m = modules.iter().find(|m| m.id == "1").unwrap();
        assert!(m.renamed.is_empty(), "{m:?}");
        assert!(m.text.contains("e.exports"), "{}", m.text);
    }
}

/// Parentheses and escaped identifier spelling do not turn eval into an indirect call.
#[test]
fn parenthesized_and_escaped_eval_keep_bindings() {
    for body in [r#"e.exports = (eval)("e");"#, r#"e.exports = ((eval))("e");"#, r#"e.exports = ev\u0061l("e");"#] {
        let modules = recover(body);
        assert!(modules.iter().find(|m| m.id == "1").unwrap().renamed.is_empty());
    }
}

/// Nested functions retain access to the factory's lexical bindings.
#[test]
fn nested_eval_keeps_outer_bindings() {
    let modules = recover(r#"e.exports = function() { return eval("e"); };"#);
    assert!(modules.iter().find(|m| m.id == "1").unwrap().renamed.is_empty());
}

/// Indirect eval and mere mentions do not inspect the factory's lexical environment.
#[test]
fn indirect_eval_and_mentions_still_rename() {
    for body in [
        r#"e.exports = (0, eval)("1");"#,
        r#"e.exports = eval?.("1");"#,
        r#"e.exports = globalThis.eval("1");"#,
        r#"e.exports = 'eval(e)'; /* eval(e) */"#,
        "e.exports = t;",
    ] {
        let modules = recover(body);
        assert_eq!(modules.iter().find(|m| m.id == "1").unwrap().renamed.len(), 3, "{body}");
    }
}
