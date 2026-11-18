//! Regression tests for TypeScript declaration filtering (`.d.mts`, `.d.cts`), NodeNext
//! `.js`/`.mjs`/`.cjs`/`.jsx` → `.ts`/`.tsx`/`.mts`/`.cts` source fallback, `.mts`/`.cts`/directory
//! `index` imports, and Electron preload TypeScript path resolution in `js_app`.

use craft_anything_analysis::js_app::{AppFile, AppGraph, AppInput, AppOptions, analyze_application, is_script};

/// Build an `AppGraph` from in-memory `(path, content)` pairs and optional extra listing entries.
fn build_graph(files: &[(&str, &str)], extra_listed: &[&str]) -> AppGraph {
    let app_files: Vec<AppFile> = files.iter().map(|(p, c)| AppFile { path: (*p).to_string(), bytes: c.as_bytes().to_vec() }).collect();
    let mut listing: Vec<(String, u64)> = app_files.iter().map(|f| (f.path.clone(), f.bytes.len() as u64)).collect();
    for p in extra_listed {
        listing.push(((*p).to_string(), 64));
    }
    let input = AppInput { label: "ts-fixture".into(), format: "directory".into(), files: app_files, listing, notes: Vec::new() };
    analyze_application(&input, &AppOptions::default(), &|| false).expect("analysis should not cancel")
}

/// `.d.ts`, `.d.mts`, and `.d.cts` declaration files are excluded from script modules and never resolved as import targets.
#[test]
fn declaration_files_d_mts_and_d_cts_are_excluded_from_scripts_and_resolved_edges() {
    assert!(!is_script("types.d.ts"));
    assert!(!is_script("TYPES.D.TS"));
    assert!(!is_script("esm.d.mts"));
    assert!(!is_script("ESM.D.MTS"));
    assert!(!is_script("cjs.d.cts"));
    assert!(!is_script("CJS.D.CTS"));
    assert!(is_script("real.mts"));
    assert!(is_script("real.cts"));

    let g = build_graph(
        &[
            ("package.json", r#"{"name":"decl-check","main":"./main.mts"}"#),
            (
                "main.mts",
                r#"
                import "./types.d.ts";
                import "./esm.d.mts";
                import "./cjs.d.cts";
                import "./UPPER.D.MTS";
                import "./shim.d.js";
                export const ok = 1;
                "#,
            ),
            (
                "index.html",
                r#"<!doctype html>
                <link rel="modulepreload" href="./cjs.d.cts">
                <link rel="modulepreload" href="./preloaded.js">
                <script type="module" src="./esm.d.mts"></script>
                "#,
            ),
            ("preloaded.ts", "export const p = 1;\n"),
            ("types.d.ts", "export interface Note { id: string; }\n"),
            ("esm.d.mts", "export declare function load(): void;\n"),
            ("cjs.d.cts", "export declare const version: string;\n"),
            ("UPPER.D.MTS", "export declare const upper: boolean;\n"),
            ("shim.d.ts", "export declare const shim: number;\n"),
            ("esm.d.mts.map", r#"{"version":3,"sources":["esm.mts"],"mappings":""}"#),
        ],
        &[],
    );

    let mut module_paths: Vec<&str> = g.modules.iter().map(|m| m.path.as_str()).collect();
    module_paths.sort_unstable();
    assert_eq!(module_paths, ["index.html", "main.mts", "preloaded.ts"]);
    let main_mod = g.module("main.mts").expect("main.mts module");
    for decl_spec in ["./types.d.ts", "./esm.d.mts", "./cjs.d.cts", "./UPPER.D.MTS", "./shim.d.js"] {
        let imp = main_mod
            .imports
            .iter()
            .find(|imp| imp.specifier == decl_spec)
            .unwrap_or_else(|| panic!("missing declaration import {decl_spec} in {:?}", main_mod.imports));
        assert_eq!(imp.resolved, None, "declaration import {decl_spec} must not resolve: {imp:?}");
    }
    let html_mod = g.module("index.html").expect("index.html module");
    assert_eq!(html_mod.imports[0].resolved, None, "html-script to .d.mts must not resolve");
    assert_eq!(html_mod.imports[1].resolved, None, "modulepreload to .d.cts must not resolve");
    assert_eq!(html_mod.imports[2].resolved.as_deref(), Some("preloaded.ts"));
    assert!(g.edges.iter().all(|e| !e.to.to_ascii_lowercase().contains(".d.")), "edges must not target declaration files: {:?}", g.edges);
}

/// TypeScript NodeNext `.js`, `.mjs`, `.cjs`, and `.jsx` specifiers resolve to `.ts`/`.tsx`/`.mts`/`.cts` sources while preferring exact matches and `.ts` before `.tsx`.
#[test]
fn typescript_nodenext_js_mjs_cjs_and_jsx_specifiers_resolve_to_source_counterparts() {
    let g = build_graph(
        &[
            ("package.json", r#"{"name":"ts-nodenext","main":"./src/main.js"}"#),
            (
                "src/main.ts",
                r#"
                import "./service.js";
                import "./View.js";
                import "./helper.mjs";
                import "./Widget.jsx";
                import "./both.js";
                import "./prefer_ts.js";
                const legacy = require("./legacy.cjs");
                new Worker(new URL("./bg.mjs", import.meta.url));
                "#,
            ),
            ("src/service.ts", "export const svc = 1;\n"),
            ("src/View.tsx", "export const View = () => <div/>;\n"),
            ("src/helper.mts", "export const h = 2;\n"),
            ("src/legacy.cts", "module.exports = { ok: true };\n"),
            ("src/Widget.tsx", "export const Widget = () => <span/>;\n"),
            ("src/both.js", "export const built = true;\n"),
            ("src/both.ts", "export const unbuilt = true;\n"),
            ("src/prefer_ts.ts", "export const tsFirst = 1;\n"),
            ("src/prefer_ts.tsx", "export const tsxSecond = 2;\n"),
            ("src/bg.mts", "self.onmessage = () => {};\n"),
        ],
        &[],
    );

    assert_eq!(g.entry_points.main.as_deref(), Some("src/main.ts"));
    assert_eq!(g.entry_points.workers, vec!["src/bg.mts"]);

    for expected_pkg in
        ["src/main.ts", "src/service.ts", "src/View.tsx", "src/helper.mts", "src/legacy.cts", "src/Widget.tsx", "src/both.js", "src/prefer_ts.ts"]
    {
        assert_eq!(g.module(expected_pkg).expect(expected_pkg).role, "package", "role mismatch on {expected_pkg}");
    }
    assert_eq!(g.module("src/both.ts").expect("src/both.ts").role, "unknown", "exact src/both.js must win over src/both.ts");
    assert_eq!(g.module("src/prefer_ts.tsx").expect("src/prefer_ts.tsx").role, "unknown", ".ts must win before .tsx");
    assert_eq!(g.module("src/bg.mts").expect("src/bg.mts").role, "worker");
}

/// Extensionless and directory `index` relative imports resolve `.mts`, `.cts`, and `index.*` while keeping compiled `index.mjs` ahead of `index.ts`.
#[test]
fn extensionless_and_directory_index_imports_resolve_mts_cts_tsx_jsx_and_cjs() {
    let g = build_graph(
        &[
            ("package.json", r#"{"name":"extless","main":"./entry.ts"}"#),
            (
                "entry.ts",
                r#"
                import "./bare_mts";
                const b = require("./bare_cts");
                const c = require("./dir_cjs");
                import "./dir_tsx";
                import "./dir_mts";
                const d = require("./dir_cts");
                import "./dir_jsx";
                import "./dir_both";
                "#,
            ),
            ("bare_mts.mts", "export const a = 1;\n"),
            ("bare_cts.cts", "module.exports = 2;\n"),
            ("dir_cjs/index.cjs", "module.exports = 3;\n"),
            ("dir_tsx/index.tsx", "export const C = () => <p/>;\n"),
            ("dir_mts/index.mts", "export const m = 4;\n"),
            ("dir_cts/index.cts", "module.exports = 5;\n"),
            ("dir_jsx/index.jsx", "export const J = () => <b/>;\n"),
            ("dir_both/index.mjs", "export const compiled = 6;\n"),
            ("dir_both/index.ts", "export const unbuilt = 7;\n"),
        ],
        &[],
    );

    for target in [
        "bare_mts.mts",
        "bare_cts.cts",
        "dir_cjs/index.cjs",
        "dir_tsx/index.tsx",
        "dir_mts/index.mts",
        "dir_cts/index.cts",
        "dir_jsx/index.jsx",
        "dir_both/index.mjs",
    ] {
        assert_eq!(g.module(target).expect(target).role, "package", "missing package role for {target}");
        assert!(
            g.edges.iter().any(|e| e.from == "module:entry.ts" && e.to == format!("module:{target}")),
            "missing dependency edge to {target}: {:?}",
            g.edges
        );
    }
    assert_eq!(g.module("dir_both/index.ts").expect("dir_both/index.ts").role, "unknown");
}

/// Electron `webPreferences.preload` prefers exact paths across `module_dir` and `root` before TypeScript fallbacks and rejects exact/cross-extension tail ambiguity and declarations.
#[test]
fn electron_preload_resolves_compiled_paths_to_typescript_sources() {
    let g = build_graph(
        &[
            ("package.json", r#"{"name":"el-ts","main":"./src/main/index.js","devDependencies":{"electron":"^33.0.0"}}"#),
            (
                "src/main/index.ts",
                r#"
                import { BrowserWindow, ipcMain } from "electron";
                import path from "node:path";

                ipcMain.handle("notes:ping", () => "pong");

                const w1 = new BrowserWindow({
                    webPreferences: { preload: path.join(__dirname, "../preload/index.js") }
                });
                w1.loadFile("renderer/index.html");

                const w2 = new BrowserWindow({
                    webPreferences: { preload: "bridge.mjs" }
                });
                const w3 = new BrowserWindow({
                    webPreferences: { preload: "legacy_preload.cjs" }
                });
                const w4 = new BrowserWindow({
                    webPreferences: { preload: "../preload/both_preload.js" }
                });
                const w5 = new BrowserWindow({
                    webPreferences: { preload: "root_exact.js" }
                });
                const w6 = new BrowserWindow({
                    webPreferences: { preload: "ambig.js" }
                });
                const w7 = new BrowserWindow({
                    webPreferences: { preload: "cross_ambig.js" }
                });
                const w8 = new BrowserWindow({
                    webPreferences: { preload: "exact_dup.js" }
                });
                const w9 = new BrowserWindow({
                    webPreferences: { preload: "local_decl.d.js" }
                });
                const w10 = new BrowserWindow({
                    webPreferences: { preload: "decl_only.d.js" }
                });
                const w11 = new BrowserWindow({
                    webPreferences: { preload: "UPPER_DECL.D.CTS" }
                });
                "#,
            ),
            (
                "src/preload/index.ts",
                r#"
                import { contextBridge, ipcRenderer } from "electron";
                contextBridge.exposeInMainWorld("api", {
                    ping: () => ipcRenderer.invoke("notes:ping"),
                });
                "#,
            ),
            ("packages/bridge/bridge.mts", "import { contextBridge } from 'electron'; contextBridge.exposeInMainWorld('b', {});\n"),
            ("src/main/legacy_preload.cts", "const { contextBridge } = require('electron'); contextBridge.exposeInMainWorld('c', {});\n"),
            ("src/preload/both_preload.js", "const { contextBridge } = require('electron'); contextBridge.exposeInMainWorld('d', {});\n"),
            ("src/preload/both_preload.ts", "import { contextBridge } from 'electron'; contextBridge.exposeInMainWorld('d_ts', {});\n"),
            ("root_exact.js", "const { contextBridge } = require('electron'); contextBridge.exposeInMainWorld('root_js', {});\n"),
            ("src/main/root_exact.ts", "import { contextBridge } from 'electron'; contextBridge.exposeInMainWorld('local_ts', {});\n"),
            ("a/ambig.ts", "export const a = 1;\n"),
            ("b/ambig.ts", "export const b = 2;\n"),
            ("a/cross_ambig.ts", "export const ca = 1;\n"),
            ("b/cross_ambig.tsx", "export const cb = () => <div/>;\n"),
            ("a/exact_dup.js", "export const e1 = 1;\n"),
            ("b/exact_dup.js", "export const e2 = 2;\n"),
            ("c/exact_dup.ts", "export const e3 = 3;\n"),
            ("src/main/local_decl.d.ts", "export declare const localDecl: number;\n"),
            ("nested/decl_only.d.ts", "export declare const x: number;\n"),
            ("nested/UPPER_DECL.D.CTS", "export declare const u: number;\n"),
            ("renderer/index.html", r#"<!doctype html><script type="module" src="./app.jsx"></script>"#),
            ("renderer/app.tsx", "window.api.ping();\n"),
        ],
        &[],
    );

    assert_eq!(g.app_kind, "electron");
    assert_eq!(g.entry_points.main.as_deref(), Some("src/main/index.ts"));
    assert_eq!(
        g.entry_points.preloads,
        vec!["src/preload/index.ts", "packages/bridge/bridge.mts", "src/main/legacy_preload.cts", "src/preload/both_preload.js", "root_exact.js",]
    );
    assert_eq!(g.module("src/preload/index.ts").expect("preload ts").role, "preload");
    assert_eq!(g.module("packages/bridge/bridge.mts").expect("bridge mts").role, "preload");
    assert_eq!(g.module("src/main/legacy_preload.cts").expect("legacy cts").role, "preload");
    assert_eq!(g.module("src/preload/both_preload.js").expect("both preload js").role, "preload");
    assert_eq!(g.module("src/preload/both_preload.ts").expect("both preload ts").role, "unknown");
    assert_eq!(g.module("root_exact.js").expect("root exact js").role, "preload");
    assert_eq!(g.module("src/main/root_exact.ts").expect("module_dir ts").role, "unknown");
    assert_eq!(g.module("renderer/app.tsx").expect("renderer tsx").role, "renderer");
    for name in ["w6", "w7", "w8", "w9", "w10", "w11"] {
        let win =
            g.windows.iter().find(|win| win["binding"].as_str() == Some(name)).unwrap_or_else(|| panic!("missing rejected window {name} in {:?}", g.windows));
        assert!(win["preload"].is_null(), "window {name} preload must stay unresolved: {win:?}");
    }

    let ch = g.ipc.iter().find(|c| c.channel == "notes:ping").expect("notes:ping channel");
    assert!(ch.paired);
}

/// Ordinary JavaScript and JSON resolution behavior remains unchanged.
#[test]
fn ordinary_javascript_and_json_resolution_control() {
    let g = build_graph(
        &[
            ("package.json", r#"{"name":"plain-js","main":"index.js"}"#),
            ("index.js", "require('./lib');\nrequire('./config.json');\n"),
            ("lib/index.js", "module.exports = 1;\n"),
            ("config.json", r#"{"ok":true}"#),
        ],
        &[],
    );

    assert_eq!(g.entry_points.main.as_deref(), Some("index.js"));
    assert_eq!(g.module("lib/index.js").expect("lib/index.js").role, "package");
    let main_mod = g.module("index.js").expect("index.js");
    assert_eq!(main_mod.imports[0].resolved.as_deref(), Some("lib/index.js"));
    assert_eq!(main_mod.imports[1].resolved.as_deref(), Some("config.json"));
}

/// Exact tails win (and reject multi-hit exact tails) before TypeScript fallbacks, and query/fragment
/// suffixes are stripped before tail lookups while preserving local TypeScript source resolution.
#[test]
fn resolve_path_prefers_exact_tail_before_ts_fallback_and_strips_query_for_tail() {
    let g = build_graph(
        &[
            ("package.json", r#"{"name":"tail-order","main":"./src/main/index.js","devDependencies":{"electron":"^33.0.0"}}"#),
            (
                "src/main/index.ts",
                r#"
                import { BrowserWindow } from "electron";
                const exactTail = new BrowserWindow({ webPreferences: { preload: "bridge.js" } });
                const dupExact = new BrowserWindow({ webPreferences: { preload: "dup_with_sibling.js" } });
                const tailQuery = new BrowserWindow({ webPreferences: { preload: "./boot.js?v=2#frag" } });
                const localTs = new BrowserWindow({ webPreferences: { preload: "local_ts_only.js" } });
                "#,
            ),
            ("src/main/bridge.ts", "export const localSibling = 1;\n"),
            ("pkg/bridge.js", "export const exactTail = 2;\n"),
            ("src/main/dup_with_sibling.ts", "export const siblingTs = 1;\n"),
            ("a/dup_with_sibling.js", "export const dupA = 1;\n"),
            ("b/dup_with_sibling.js", "export const dupB = 2;\n"),
            ("packages/ui/boot.ts", "export const booted = true;\n"),
            ("src/main/local_ts_only.ts", "export const localOnly = true;\n"),
            ("other/local_ts_only.ts", "export const otherTs = true;\n"),
        ],
        &[],
    );

    let win = |binding: &str| {
        g.windows.iter().find(|w| w["binding"].as_str() == Some(binding)).unwrap_or_else(|| panic!("missing window {binding}: {:?}", g.windows))
    };
    assert_eq!(win("exactTail")["preload"].as_str(), Some("pkg/bridge.js"));
    assert!(win("dupExact")["preload"].is_null(), "ambiguous exact tail must stay unresolved: {:?}", win("dupExact"));
    assert_eq!(win("tailQuery")["preload"].as_str(), Some("packages/ui/boot.ts"));
    assert_eq!(win("localTs")["preload"].as_str(), Some("src/main/local_ts_only.ts"));
}

/// Scheme-relative HTML references (`//cdn.example/...`) are never resolved to local files, query URLs
/// (`?origin=https://...`) stay eligible as local paths, and unresolved bare HTML paths do not create
/// `external:*` package nodes while real package imports still do.
#[test]
fn html_refs_reject_scheme_relative_preserve_query_urls_and_skip_external_nodes() {
    let g = build_graph(
        &[
            (
                "index.html",
                r#"<!doctype html>
                <link rel="modulepreload" href="//cdn.example/chunk.js">
                <link rel="modulepreload" href="./preload_ok.js?origin=https://example.test">
                <link rel="modulepreload" href="missing_preload.js">
                <script type="module" src="//cdn.example/boot.js"></script>
                <script type="module" src="./script_ok.js?origin=https://example.test"></script>
                <script type="module" src="bare.d.mts"></script>
                "#,
            ),
            ("cdn.example/boot.ts", "export const unrelatedBoot = 1;\n"),
            ("cdn.example/chunk.ts", "export const unrelatedChunk = 1;\n"),
            ("preload_ok.js", "export const preloadOk = 1;\n"),
            ("script_ok.ts", "import 'react';\nexport const scriptOk = 1;\n"),
            ("bare.d.mts", "export declare const bareDecl: number;\n"),
        ],
        &[],
    );

    let html_mod = g.module("index.html").expect("index.html module");
    let find_imp = |kind: &str, spec: &str| {
        html_mod
            .imports
            .iter()
            .find(|imp| imp.kind == kind && imp.specifier == spec)
            .unwrap_or_else(|| panic!("missing {kind} import {spec:?} in {:?}", html_mod.imports))
    };

    assert_eq!(find_imp("modulepreload", "//cdn.example/chunk.js").resolved, None);
    assert_eq!(find_imp("modulepreload", "./preload_ok.js?origin=https://example.test").resolved.as_deref(), Some("preload_ok.js"));
    assert_eq!(find_imp("modulepreload", "missing_preload.js").resolved, None);
    assert_eq!(find_imp("html-script", "./script_ok.js?origin=https://example.test").resolved.as_deref(), Some("script_ok.ts"));
    assert_eq!(find_imp("html-script", "bare.d.mts").resolved, None);
    assert!(
        !html_mod.imports.iter().any(|imp| imp.specifier == "//cdn.example/boot.js"),
        "scheme-relative HTML script should be skipped before local resolution: {:?}",
        html_mod.imports
    );

    assert_eq!(g.module("cdn.example/boot.ts").expect("cdn.example/boot.ts").role, "unknown");
    assert_eq!(g.module("cdn.example/chunk.ts").expect("cdn.example/chunk.ts").role, "unknown");
    assert_eq!(g.module("preload_ok.js").expect("preload_ok.js").role, "page");
    assert_eq!(g.module("script_ok.ts").expect("script_ok.ts").role, "page");

    let external_ids: Vec<&str> = g.nodes.iter().filter(|n| n.kind == "external").map(|n| n.id.as_str()).collect();
    assert_eq!(external_ids, ["external:react"], "bare HTML paths must not become external package nodes: {external_ids:?}");
}

/// HTML refs with a URL scheme but no `//` after it (`data:`) are external, while a local path whose
/// query carries a URL stays local and a path climbing above the app root resolves nowhere.
#[test]
fn html_refs_with_any_url_scheme_are_external() {
    let g = build_graph(
        &[
            (
                "index.html",
                r#"<!doctype html>
                <link rel="modulepreload" href="data:text/javascript,0//../../boot.js">
                <script type="module" src="data:text/javascript,0//../../boot.js?origin=https://example.test"></script>
                <script type="module" src="./local.js?origin=https://example.test"></script>
                <script type="module" src="a/../../shared/x.js"></script>
                "#,
            ),
            ("boot.js", "export const boot = 1;\n"),
            ("local.js", "export const local = 1;\n"),
            ("lib/shared/x.js", "export const x = 1;\n"),
        ],
        &[],
    );
    let html = g.module("index.html").expect("index.html module");
    let data: Vec<_> = html.imports.iter().filter(|i| i.specifier.starts_with("data:")).map(|i| (i.kind.as_str(), i.resolved.as_deref())).collect();
    assert_eq!(data, [("modulepreload", None)], "a data: script is skipped and a data: preload resolves nowhere: {:?}", html.imports);
    let resolved = |spec: &str| html.imports.iter().find(|i| i.specifier == spec).unwrap_or_else(|| panic!("missing {spec:?}")).resolved.clone();
    assert_eq!(resolved("./local.js?origin=https://example.test").as_deref(), Some("local.js"));
    assert_eq!(resolved("a/../../shared/x.js"), None);
}

/// `./chunks/./bridge.js` is the same tail as `./chunks/bridge.js`, so both reach the compiled file.
#[test]
fn equivalent_spellings_reach_the_same_exact_tail() {
    let g = build_graph(
        &[
            (
                "ui/index.html",
                r#"<!doctype html>
                <script type="module" src="./chunks/bridge.js"></script>
                <script type="module" src="./chunks/./bridge.js"></script>
                "#,
            ),
            ("ui/chunks/bridge.ts", "export const source = 1;\n"),
            ("dist/chunks/bridge.js", "export const compiled = 1;\n"),
        ],
        &[],
    );
    let html = g.module("ui/index.html").expect("ui/index.html module");
    let resolved: Vec<_> = html.imports.iter().filter(|i| i.kind == "html-script").map(|i| (i.specifier.as_str(), i.resolved.as_deref())).collect();
    assert_eq!(resolved, [("./chunks/bridge.js", Some("dist/chunks/bridge.js")), ("./chunks/./bridge.js", Some("dist/chunks/bridge.js"))]);
}

/// A dependency file under `node_modules/` that only appears in the listing never wins an exact tail
/// over the page's own TypeScript source.
#[test]
fn node_modules_listing_entries_never_win_an_exact_tail() {
    let g = build_graph(
        &[("ui/index.html", r#"<!doctype html><script type="module" src="./boot.js"></script>"#), ("ui/boot.ts", "export const boot = 1;\n")],
        &["node_modules/widget/boot.js"],
    );
    let html = g.module("ui/index.html").expect("ui/index.html module");
    let boot = html.imports.iter().find(|i| i.specifier == "./boot.js").expect("./boot.js import");
    assert_eq!(boot.resolved.as_deref(), Some("ui/boot.ts"));
    assert_eq!(g.module("ui/boot.ts").expect("ui/boot.ts").role, "page");
}

/// HTML `src`/`href` values are read as a browser reads them, with surrounding spaces and controls
/// removed, so a spaced remote URL stays external and a spaced local path still resolves.
#[test]
fn html_refs_ignore_surrounding_spaces_and_controls() {
    let g = build_graph(
        &[
            (
                "index.html",
                "<!doctype html>
                <link rel=\"modulepreload\" href=\" https://example.test/../../boot.js\">
                <script type=\"module\" src=\" https://example.test/../../boot.js\"></script>
                <script type=\"module\" src=\"\t./local.js \"></script>
                ",
            ),
            ("boot.js", "export const boot = 1;\n"),
            ("local.js", "export const local = 1;\n"),
        ],
        &[],
    );
    let html = g.module("index.html").expect("index.html module");
    let found: Vec<_> = html.imports.iter().map(|i| (i.kind.as_str(), i.resolved.as_deref())).collect();
    assert_eq!(found, [("html-script", Some("local.js")), ("modulepreload", None)], "{:?}", html.imports);
}

/// Only a whole `node_modules` directory segment marks dependency code: `my_node_modules/` is the
/// app's own folder and can win an exact tail, while `dist/node_modules/` cannot.
#[test]
fn only_whole_node_modules_segments_are_dependency_code() {
    let own = build_graph(
        &[("ui/index.html", r#"<!doctype html><script type="module" src="./boot.js"></script>"#), ("dist/my_node_modules/boot.js", "export const own = 1;\n")],
        &[],
    );
    let html = own.module("ui/index.html").expect("ui/index.html module");
    assert_eq!(html.imports[0].resolved.as_deref(), Some("dist/my_node_modules/boot.js"));
    assert!(own.module("dist/my_node_modules/boot.js").is_some(), "the app's own folder is analysed");

    let dep = build_graph(
        &[("ui/index.html", r#"<!doctype html><script type="module" src="./boot.js"></script>"#), ("dist/node_modules/boot.js", "export const dep = 1;\n")],
        &[],
    );
    assert_eq!(dep.module("ui/index.html").expect("ui/index.html module").imports[0].resolved, None);
}

/// A window's remote `loadURL` target stays a URL even when its path normalises onto a local page;
/// a `file:` target is still local.
#[test]
fn remote_window_loads_stay_urls() {
    let g = build_graph(
        &[
            ("package.json", r#"{"name":"remote-load","main":"main.js","devDependencies":{"electron":"^33.0.0"}}"#),
            (
                "main.js",
                r#"const { BrowserWindow } = require("electron");
                const remote = new BrowserWindow({});
                remote.loadURL("https://example.test/../../ui/index.html");
                "#,
            ),
            ("ui/index.html", "<!doctype html><p>local</p>"),
        ],
        &[],
    );
    let win = g.windows.iter().find(|w| w["binding"] == "remote").expect("remote window");
    assert_eq!(win["loads"], serde_json::json!(["https://example.test/../../ui/index.html"]), "{win}");
    assert!(!g.edges.iter().any(|e| e.kind == "loads" && e.to == "module:ui/index.html"), "{:?}", g.edges);
}

/// URL schemes are case-insensitive: `FILE:///ui/index.html` is the same local page as
/// `file:///ui/index.html`. `localhost` names this computer, so `FILE://localhost/ui/index.html`
/// is that page too, not `localhost/ui/index.html`.
#[test]
fn file_url_loads_resolve_in_any_letter_case() {
    let g = build_graph(
        &[
            ("package.json", r#"{"name":"file-load","main":"main.js","devDependencies":{"electron":"^33.0.0"}}"#),
            (
                "main.js",
                r#"const { BrowserWindow } = require("electron");
                const upper = new BrowserWindow({});
                upper.loadURL("FILE:///ui/index.html");
                const lower = new BrowserWindow({});
                lower.loadURL("file:///ui/settings.html");
                const local = new BrowserWindow({});
                local.loadURL("FILE://LocalHost/ui/index.html");
                "#,
            ),
            ("ui/index.html", "<!doctype html><p>main</p>"),
            ("ui/settings.html", "<!doctype html><p>settings</p>"),
            ("localhost/ui/index.html", "<!doctype html><p>a folder named localhost</p>"),
        ],
        &[],
    );
    for (binding, page) in [("upper", "ui/index.html"), ("lower", "ui/settings.html"), ("local", "ui/index.html")] {
        let win = g.windows.iter().find(|w| w["binding"] == binding).expect("window");
        assert_eq!(win["loads"], serde_json::json!([page]), "{win}");
        let from = format!("window:{}@{}", win["module"].as_str().unwrap(), win["line"]);
        let loads: Vec<_> = g.edges.iter().filter(|e| e.kind == "loads" && e.from == from).map(|e| e.to.as_str()).collect();
        assert_eq!(loads, [format!("module:{page}")], "{binding}: {:?}", g.edges);
    }
    assert!(!g.edges.iter().any(|e| e.to.starts_with("url:")), "{:?}", g.edges);
}

/// A backslash after the local authority starts a path at the inventory root.
#[test]
fn file_url_backslash_load_uses_the_root_page() {
    let g = build_graph(
        &[
            ("dist/main.js", r#"const w = new BrowserWindow({}); w.loadURL("file://localhost\\ui\\index.html");"#),
            ("ui/index.html", "<p>root</p>"),
            ("dist/ui/index.html", "<p>beside main</p>"),
            ("dist/localhost/ui/index.html", "<p>host as folder</p>"),
        ],
        &[],
    );
    let w = g.windows.first().expect("window");
    assert_eq!(w["loads"], serde_json::json!(["ui/index.html"]));
    assert!(g.edges.iter().any(|e| e.kind == "loads" && e.to == "module:ui/index.html"));
}

/// A local file URL with no path names the root even with a query or fragment.
#[test]
fn file_url_root_loads_never_reach_a_typescript_file() {
    for url in ["file://localhost", "file://localhost?x", "file://localhost#x", "file:///", "file://"] {
        let main = format!(r#"const w = new BrowserWindow({{}}); w.loadURL("{url}");"#);
        let g = build_graph(&[("dist.js/main.js", &main), ("dist.ts", "")], &["dist.js/localhost"]);
        let w = g.windows.first().expect("window");
        assert_eq!(w["loads"], serde_json::json!([url]), "{url}");
        assert!(!g.edges.iter().any(|e| e.kind == "loads" && e.to.starts_with("module:")), "{url}");
    }
}

/// A browser removes ASCII tabs and newlines inside a URL before reading its scheme, so
/// `ht<TAB>tps://…` is remote; a local path with a tab inside still resolves.
#[test]
fn html_refs_drop_interior_tabs_and_newlines() {
    let g = build_graph(
        &[
            (
                "index.html",
                "<!doctype html>
                <link rel=\"modulepreload\" href=\"ht\ttps://example.test/../../boot.js\">
                <script type=\"module\" src=\"ht\ttps://example.test/../../boot.js\"></script>
                <script type=\"module\" src=\"./lo\ncal.js\"></script>
                ",
            ),
            ("boot.js", "export const boot = 1;\n"),
            ("local.js", "export const local = 1;\n"),
        ],
        &[],
    );
    let html = g.module("index.html").expect("index.html module");
    let found: Vec<_> = html.imports.iter().map(|i| (i.kind.as_str(), i.specifier.as_str(), i.resolved.as_deref())).collect();
    assert_eq!(
        found,
        [("html-script", "./lo\ncal.js", Some("local.js")), ("modulepreload", "ht\ttps://example.test/../../boot.js", None)],
        "{:?}",
        html.imports
    );
}

/// A path ending in `/`, `/.` or `/..` names a folder: `./boot.js/` and `./boot.js/.` never reach
/// `boot.js`, directly or by tail, while `./boot.js` and `./x/../boot.js` still do.
#[test]
fn trailing_slash_refs_never_reach_a_file() {
    let g = build_graph(
        &[
            ("dist/index.html", r#"<!doctype html><link rel="modulepreload" href="./boot.js/"><script type="module" src=" ./boot.js/ "></script>"#),
            ("ui/page.html", r#"<!doctype html><script type="module" src="./boot.js/?v=1"></script><script type="module" src="./boot.js"></script>"#),
            (
                "dist/dot.html",
                r#"<!doctype html><link rel="modulepreload" href="./boot.js/.?v=1"><script type="module" src="./boot.js/."></script><script type="module" src="./x/../boot.js"></script><link rel="modulepreload" href="./boot.js/child/..?v=1"><script type="module" src="./boot.js/child/..?v=1"></script>"#,
            ),
            ("dist/boot.js", "export const boot = 1;\n"),
        ],
        &[],
    );
    let resolved = |m: &str| g.module(m).expect("module").imports.iter().map(|i| (i.kind.clone(), i.resolved.clone())).collect::<Vec<_>>();
    assert_eq!(resolved("dist/index.html"), [("html-script".to_string(), None), ("modulepreload".to_string(), None)]);
    assert_eq!(resolved("ui/page.html"), [("html-script".to_string(), None), ("html-script".to_string(), Some("dist/boot.js".to_string()))]);
    assert_eq!(
        resolved("dist/dot.html"),
        [
            ("html-script".to_string(), None),
            ("html-script".to_string(), Some("dist/boot.js".to_string())),
            ("html-script".to_string(), None),
            ("modulepreload".to_string(), None),
            ("modulepreload".to_string(), None)
        ]
    );
}
