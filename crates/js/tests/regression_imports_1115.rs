use craft_anything_js::{Limits, ParseMode, analyze};
/// Bare imports survive fallback, including comments and decoded string escapes.
#[test]
fn side_effect_dependencies_survive_fallback() {
    let src = r#"import './polyfill.js'; import /* load */ "./setup\u002ejs";"#;
    let a = analyze("app.mjs", src, &Limits { max_parse_bytes: 0, ..Limits::default() });
    assert_eq!(a.facts.mode, ParseMode::Lexer);
    assert_eq!(
        a.facts.imports.iter().map(|i| (&*i.kind, &*i.specifier, i.literal)).collect::<Vec<_>>(),
        [("import", "./polyfill.js", true), ("import", "./setup.js", true)]
    );
    assert!(a.facts.imports.iter().all(|i| i.names.is_empty()));
}
/// A source-size admission refusal follows the same path as an explicit fallback.
#[test]
fn large_source_retains_side_effect_import() {
    let src = format!("/*{}*/\nimport './init.js';", "x".repeat(2048));
    let a = analyze("app.mjs", &src, &Limits { max_parse_bytes: 1024, ..Limits::default() });
    assert_eq!(a.facts.mode, ParseMode::Lexer);
    assert_eq!(a.facts.imports.len(), 1);
    assert_eq!(a.facts.imports[0].line, 2);
    assert_eq!(a.facts.imports[0].offset, src.find("import").unwrap() as u32);
}
/// Bare imports obey the same bounded import list as other import forms.
#[test]
fn side_effect_imports_are_bounded() {
    let src = "import './x.js';".repeat(5010);
    let a = analyze("app.mjs", &src, &Limits { max_parse_bytes: 0, ..Limits::default() });
    assert_eq!(a.facts.imports.len(), 5000);
    assert!(a.facts.truncated.iter().any(|t| t == "imports"), "{:?}", a.facts.truncated);
}
/// A full list says it was cut whichever import form overflowed it; exactly 5,000 does not.
#[test]
fn a_full_import_list_says_it_was_cut() {
    let lexer = Limits { max_parse_bytes: 0, ..Limits::default() };
    for (form, n, cut) in
        [("import './x.js';", 5000, false), ("import x from './x.js';", 5001, true), ("require('./x.js');", 5001, true), ("import('./x.js');", 5001, true)]
    {
        let a = analyze("app.mjs", &form.repeat(n), &lexer);
        assert_eq!(a.facts.imports.len(), 5000, "{form}");
        assert_eq!(a.facts.truncated.iter().any(|t| t == "imports"), cut, "{form} x{n}: {:?}", a.facts.truncated);
    }
}
/// A `.node` addon is a native module whichever import form loads it, as in the parsed pass.
#[test]
fn native_addons_from_every_import_form() {
    let lexer = Limits { max_parse_bytes: 0, ..Limits::default() };
    for src in ["import './addon.node';", "import addon from './addon.node';", "require('./addon.node');"] {
        for limits in [Limits::default(), lexer] {
            let a = analyze("app.mjs", src, &limits);
            assert_eq!(a.facts.native_modules, ["./addon.node"], "{src} {:?}", a.facts.mode);
        }
    }
}
/// Existing named/dynamic imports and require stay distinct from strings and comments.
#[test]
fn existing_import_forms_and_noncode() {
    let src = r#"import {x} from './named.js'; import('./lazy.js'); require('./cjs.js'); const s="import './fake.js'"; /* import './fake2.js'; */ obj.import
'./fake3.js'; obj?.import
'./fake4.js';"#;
    let a = analyze("app.mjs", src, &Limits { max_parse_bytes: 0, ..Limits::default() });
    assert_eq!(a.facts.imports.iter().map(|i| i.specifier.as_str()).collect::<Vec<_>>(), ["./named.js", "./lazy.js", "./cjs.js"]);
}
