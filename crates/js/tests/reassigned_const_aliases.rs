//! Regression tests for constant propagation and module alias tracking across reassignments,
//! compound/update writes, destructuring/loop bindings, and control-flow branches.

use craft_anything_js::{Limits, ParseMode, analyze};

/// Helper returning `fetch` endpoint URLs and literality for a source snippet.
fn fetch_endpoints(src: &str) -> Vec<(String, bool)> {
    let a = analyze("app.js", src, &Limits::default());
    assert_eq!(a.facts.mode, ParseMode::Parsed, "{:?}", a.facts.mode_reason);
    a.facts.endpoints.iter().filter(|e| e.api == "fetch").map(|e| (e.url.clone(), e.literal)).collect()
}

/// Straight-line reassignments must replace or invalidate stale constant strings.
#[test]
fn straight_line_reassignment_updates_and_invalidates_constants() {
    assert_eq!(
        fetch_endpoints(r#"let url = "https://old.example/v1"; url = "https://new.example/v2"; fetch(url);"#),
        vec![("https://new.example/v2".to_string(), true)]
    );
    assert_eq!(fetch_endpoints(r#"let url = "https://default.example"; url = computeUrl(); fetch(url);"#), vec![("{url}".to_string(), false)]);
    assert_eq!(fetch_endpoints(r#"let url = "https://api.example"; url = url + "/v2"; fetch(url);"#), vec![("https://api.example/v2".to_string(), true)]);
    assert_eq!(fetch_endpoints(r#"let url = "https://api.example"; url += "/v3"; fetch(url);"#), vec![("https://api.example/v3".to_string(), true)]);
}

/// Update operators (`++`, `--`) and non-concatenation compound assignments must invalidate constants.
#[test]
fn update_and_compound_assignments_invalidate_constants() {
    assert_eq!(fetch_endpoints(r#"let n = 1; n++; fetch("https://api.example/v" + n);"#), vec![("https://api.example/v{n}".to_string(), false)]);
    assert_eq!(fetch_endpoints(r#"let x = 2; x *= 3; fetch("https://api.example/v" + x);"#), vec![("https://api.example/v{x}".to_string(), false)]);
    assert_eq!(fetch_endpoints(r#"let u = "https://a.example"; u ||= "https://b.example"; fetch(u);"#), vec![("{u}".to_string(), false)]);
}

/// Destructuring assignments and loop variable bindings must invalidate stale constants.
#[test]
fn destructuring_and_loop_bindings_invalidate_constants() {
    assert_eq!(fetch_endpoints(r#"let url = "https://stale1.example"; [url] = list; fetch(url);"#), vec![("{url}".to_string(), false)]);
    assert_eq!(fetch_endpoints(r#"let host = "https://stale2.example"; ({ host } = cfg); fetch(host);"#), vec![("{host}".to_string(), false)]);
    assert_eq!(fetch_endpoints(r#"let item = "https://stale3.example"; for (item of items) { fetch(item); }"#), vec![("{item}".to_string(), false)]);
}

/// Writes inside conditional branches, loops, short-circuit expressions, or nested functions
/// must not leak stale or branch-only values to following code, while same-named shadowing does not interfere.
#[test]
fn conditional_loop_and_closure_writes_invalidate_outer_constants() {
    assert_eq!(
        fetch_endpoints(r#"let url = "https://init.example"; if (cond) { url = "https://branch.example"; } fetch(url);"#),
        vec![("{url}".to_string(), false)]
    );
    assert_eq!(
        fetch_endpoints(r#"let url = "https://init.example"; cond && (url = "https://short.example"); fetch(url);"#),
        vec![("{url}".to_string(), false)]
    );
    assert_eq!(
        fetch_endpoints(r#"let url = "https://init.example"; function mutate() { url = "https://later.example"; } fetch(url);"#),
        vec![("{url}".to_string(), false)]
    );
    // Shadowing inside a nested function or block leaves the outer constant intact.
    assert_eq!(
        fetch_endpoints(r#"let url = "https://outer.example"; function inner() { let url = "https://inner.example"; } fetch(url);"#),
        vec![("https://outer.example".to_string(), true)]
    );
    assert_eq!(
        fetch_endpoints(r#"let url = "https://outer.example"; if (cond) { let url = "https://inner.example"; } fetch(url);"#),
        vec![("https://outer.example".to_string(), true)]
    );
    // Hoisted `var` redeclaration inside a conditional branch invalidates the outer constant.
    assert_eq!(
        fetch_endpoints(r#"var url = "https://outer.example"; if (cond) { var url = "https://inner.example"; } fetch(url);"#),
        vec![("{url}".to_string(), false)]
    );
}

/// Module aliases must update on straight-line `require(...)` assignments and invalidate when
/// overwritten with another value or mutated inside a conditional branch.
#[test]
fn module_aliases_track_reassignment_and_invalidation() {
    let overwritten = analyze("a.js", r#"let modFs = require("fs"); modFs = fakeHelper; modFs.readFile("a.txt");"#, &Limits::default());
    assert!(overwritten.facts.effects.is_empty(), "overwritten alias must not emit fs.readFile: {:?}", overwritten.facts.effects);

    let deferred = analyze(
        "b.js",
        r#"let fsMod; fsMod = require("fs"); fsMod.writeFileSync("b.txt", "x"); let ipc; ipc = require("electron").ipcMain; ipc.handle("ch", () => {});"#,
        &Limits::default(),
    );
    assert!(deferred.facts.effects.iter().any(|e| e.api == "fs.writeFileSync"), "{:?}", deferred.facts.effects);
    assert!(deferred.facts.ipc.iter().any(|i| i.side == "main" && i.channel.as_deref() == Some("ch")), "{:?}", deferred.facts.ipc);

    let conditional = analyze("c.js", r#"let modFs = require("fs"); if (mock) { modFs = mockFs; } modFs.readFile("c.txt");"#, &Limits::default());
    assert!(conditional.facts.effects.is_empty(), "conditionally overwritten alias must be invalidated: {:?}", conditional.facts.effects);

    let destructured_redecl = analyze(
        "d.js",
        r#"const helpers = [fakeHelper]; var modFs; modFs = require("fs"); var [modFs] = helpers; modFs.readFile("d.txt");"#,
        &Limits::default(),
    );
    assert!(destructured_redecl.facts.effects.is_empty(), "destructured redeclaration must clear prior module alias: {:?}", destructured_redecl.facts.effects);
}

/// Logical assignment operators (`||=`, `&&=`, `??=`) evaluate their right-hand side
/// conditionally, and `try` writes must be invalidated before visiting `catch`.
#[test]
fn logical_assignments_and_try_catch_invalidate_conditional_writes() {
    assert_eq!(fetch_endpoints(r#"let url = "/old"; let ready = true; ready ||= (url = "/new"); fetch(url);"#), vec![("{url}".to_string(), false)]);
    assert_eq!(fetch_endpoints(r#"let url = "/old"; let ready = false; ready &&= (url = "/new"); fetch(url);"#), vec![("{url}".to_string(), false)]);
    assert_eq!(fetch_endpoints(r#"let url = "/old"; let ready = "set"; ready ??= (url = "/new"); fetch(url);"#), vec![("{url}".to_string(), false)]);
    assert_eq!(fetch_endpoints(r#"var url = "/old"; try { var url = "/new"; throw 0; } catch { fetch(url); }"#), vec![("{url}".to_string(), false)]);
}
