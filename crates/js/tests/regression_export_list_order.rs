//! `export { f }` marks the function it names as exported. The list was matched against the
//! functions seen so far, by name, so a list before the declaration marked nothing and one after it
//! also marked nested functions that only shared the name.

use craft_anything_js::{Analysis, Limits, ParseMode, analyze};

fn parsed(src: &str) -> Analysis {
    let analysis = analyze("app.mjs", src, &Limits::default());
    assert_eq!(analysis.facts.mode, ParseMode::Parsed, "{:?}", analysis.facts.mode_reason);
    analysis
}

/// `(name, parent, exported)` for every function.
fn functions(a: &Analysis) -> Vec<(String, Option<String>, bool)> {
    a.facts.functions.iter().map(|f| (f.name.clone(), f.parent.clone(), f.exported)).collect()
}

fn exported(a: &Analysis, name: &str) -> bool {
    a.facts.functions.iter().any(|f| f.name == name && f.parent.is_none() && f.exported)
}

#[test]
fn a_list_before_the_declaration() {
    let a = parsed("export { f };\nfunction f() { return 1; }\nfunction g() {}\n");
    assert!(exported(&a, "f"), "{:?}", functions(&a));
    assert!(!exported(&a, "g"), "{:?}", functions(&a));
}

#[test]
fn a_renamed_list_before_bound_expressions() {
    let a = parsed("export { f as run, h as default };\nconst f = () => 1;\nvar h = function named() {};\n");
    assert!(exported(&a, "f"), "{:?}", functions(&a));
    assert!(exported(&a, "h"), "{:?}", functions(&a));
}

/// Only the module-level `f` is exported; a nested function of the same name is a different one.
#[test]
fn nested_functions_with_the_same_name_are_not_exported() {
    for src in [
        "function outer() { function f() {} return f; }\nfunction f() {}\nexport { f };\n",
        "export { f };\nfunction outer() { const f = () => 2; return f; }\nfunction f() {}\n",
        "function outer() { return [1].map(function f() {}); }\nfunction f() {}\nexport { f };\n",
    ] {
        let a = parsed(src);
        let all = functions(&a);
        assert!(exported(&a, "f"), "{all:?}");
        let nested = all.iter().find(|(n, p, _)| n == "f" && p.is_some()).expect("the nested f");
        assert!(!nested.2, "{src}: {all:?}");
    }
}

/// Module-level functions that are not the binding but share its name (an object method, a
/// named callback) are not exported either, wherever the list is.
#[test]
fn unbound_functions_with_the_same_name_are_not_exported() {
    for src in [
        "export { setup };\nfunction setup() {}\ndefineComponent({ setup() { return {}; } });\n",
        "defineComponent({ setup() { return {}; } });\nfunction setup() {}\nexport { setup };\n",
        "const handler = () => 1;\napp.use(function handler() {});\nexport { handler };\n",
    ] {
        let a = parsed(src);
        let marked = a.facts.functions.iter().filter(|f| f.exported).count();
        assert_eq!(marked, 1, "{src}: {:?}", functions(&a));
    }
}

/// A function assigned to the listed variable is exported, also when the assignment is inside
/// another function.
#[test]
fn functions_assigned_to_the_binding() {
    for src in [
        "let handler;\nfunction init() { handler = () => 1; }\ninit();\nexport { handler };\n",
        "export { handler };\nlet handler;\n(function () { handler = function () {}; })();\n",
    ] {
        let a = parsed(src);
        let marked: Vec<_> = a.facts.functions.iter().filter(|f| f.exported).map(|f| f.name.as_str()).collect();
        assert_eq!(marked, ["handler"], "{src}: {:?}", functions(&a));
    }
}

/// Lists after the declaration, and functions given to a name later, read as before.
#[test]
fn lists_after_the_declaration_still_work() {
    let a = parsed("function f() {}\nconst h = function () {};\nexport { f, h };\n");
    assert!(exported(&a, "f") && exported(&a, "h"), "{:?}", functions(&a));
    let a = parsed("let f;\nf = function () {};\nexport { f };\n");
    assert!(exported(&a, "f"), "{:?}", functions(&a));
    let a = parsed("export function f() {}\nfunction g() {}\n");
    assert!(exported(&a, "f") && !exported(&a, "g"), "{:?}", functions(&a));
}
