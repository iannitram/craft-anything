//! Node and TypeScript fix the module system by extension: `.cts` is CommonJS and `.mts` is an
//! ES module whatever the file contains. Both used to be read like `.ts`, where the syntax decides,
//! so a `.cts` file that calls `require` was a script and a `.mts` file without `import` was too.

use craft_anything_js::{Analysis, Limits, ParseMode, analyze};

/// Analyse `src` as `path` and check the syntax tree was used without errors.
fn parsed(path: &str, src: &str) -> Analysis {
    let analysis = analyze(path, src, &Limits::default());
    assert_eq!(analysis.facts.mode, ParseMode::Parsed, "{path}: {:?}", analysis.facts.mode_reason);
    analysis
}

const REQUIRE: &str = "const fs = require('fs');\nmodule.exports = { read: (p: string) => fs.readFileSync(p) };\n";

#[test]
fn cts_is_commonjs_like_cjs() {
    assert_eq!(parsed("lib/index.cts", REQUIRE).facts.source_kind, "commonjs");
    assert_eq!(parsed("LIB/INDEX.CTS", REQUIRE).facts.source_kind, "commonjs");
    // The JavaScript counterpart, for comparison.
    assert_eq!(parsed("lib/index.cjs", "const fs = require('fs');\n").facts.source_kind, "commonjs");
}

/// TypeScript writes CommonJS imports with `import` syntax and compiles them to `require`, so a
/// `.cts` file with `import` and `export` stays CommonJS, parses cleanly and keeps its imports.
#[test]
fn cts_with_typescript_import_syntax() {
    let src = "import fs from 'fs';\nimport path = require('path');\nexport const x: number = 1;\nexport = x;\n";
    let a = parsed("tool.cts", src);
    assert_eq!(a.facts.source_kind, "commonjs");
    let specs: Vec<&str> = a.facts.imports.iter().map(|i| i.specifier.as_str()).collect();
    assert!(specs.contains(&"fs"), "{specs:?}");
    assert!(a.facts.exports.esm.contains(&"x".to_string()), "{:?}", a.facts.exports);
    let plain = parsed("tool.cts", "import fs from 'fs';\nfs.readFileSync('a');\n");
    assert_eq!(plain.facts.source_kind, "commonjs");
}

#[test]
fn mts_is_a_module_without_import_syntax() {
    assert_eq!(parsed("main.mts", "const x: number = 1;\nconsole.log(x);\n").facts.source_kind, "module");
    // Top-level await is valid only in a module.
    assert_eq!(parsed("main.mts", "const r = await fetch('/api');\n").facts.source_kind, "module");
    assert_eq!(parsed("main.mts", "import fs from 'fs';\n").facts.source_kind, "module");
}

/// `.ts` and `.tsx` still let the syntax decide.
#[test]
fn ts_is_still_decided_by_syntax() {
    assert_eq!(parsed("a.ts", REQUIRE).facts.source_kind, "script");
    assert_eq!(parsed("a.ts", "import fs from 'fs';\n").facts.source_kind, "module");
    assert_eq!(parsed("a.tsx", "export const A = () => <div />;\n").facts.source_kind, "module");
    assert_eq!(parsed("a.d.ts", "declare const x: number;\n").facts.source_kind, "script");
}
