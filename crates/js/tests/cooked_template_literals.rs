//! Regression tests for cooked template-literal values across JS fact and bundle extraction.

use craft_anything_js::{Limits, analyze};

/// `require(...)`, `(require)(...)`, computed member chains, `await import(...)`, and `require("bindings")(...)`
/// decode escaped no-substitution template literals so module aliases, effects, IPC, and native modules survive.
#[test]
fn require_and_dynamic_import_decode_cooked_templates_and_propagate_aliases() {
    let src = r#"
        const fs = require(`node\x3afs`);
        fs.readFileSync(`/etc/hosts`);
        const ipc = (require)(`elec\x74ron`)[`ipc\x52enderer`];
        ipc[`in\x76oke`](`note\x3asave`);
        async function run() {
            const cp = await import(`node\x3achild_process`);
            cp.exec(`id`);
            (await import(`node\x3afs`)).chmodSync(`/tmp/x`, 0o600);
            const seqFs = await (0, import(`node\x3afs`));
            seqFs.writeFileSync(`/tmp/y`, `ok`);
            (await (0, (1, import(`node\x3afs`)))).unlinkSync(`/tmp/z`);
            const multiAwait = await (0, await (1, import(`node\x3afs`)));
            multiAwait.rmdirSync(`/tmp/d`);
            const nonTailSeq = await (import(`node\x3afs`), noop());
            nonTailSeq.symlinkSync(`/a`, `/b`);
            await import(`node\x3afs`).then(onAwaitedThen);
            const seqThen = await (0, import(`node\x3afs`).then(onAwaitedSeqThen));
            seqThen.watchFile(`/tmp/w`);
            (await (0, import(`node\x3afs`)).then(onOuterThen));
        }
        require(`bind\x69ngs`)(`native\x2daddon`);
        const pendingFs = import(`node\x3afs`);
        pendingFs.then(onLoaded);
        const chainedFs = import(`node\x3afs`).then(m => m);
        chainedFs.then(useModule);
        import(`node\x3afs`).then(onInlineThen);
        (0, import(`node\x3afs`)).then(onSeqThen);
        const seqUnawaited = (0, import(`node\x3afs`));
        seqUnawaited.then(onSeqUnawaited);
        const pendingStore = import(`electron\x2dstore`);
        pendingStore.then(onStore);
    "#;

    let a = analyze("main.js", src, &Limits::default());
    let specs: Vec<&str> = a.facts.imports.iter().map(|i| i.specifier.as_str()).collect();
    assert!(specs.contains(&"node:fs"), "imports: {specs:?}");
    assert!(specs.contains(&"electron"), "imports: {specs:?}");
    assert!(specs.contains(&"node:child_process"), "imports: {specs:?}");

    let effect_apis: Vec<&str> = a.facts.effects.iter().map(|e| e.api.as_str()).collect();
    assert!(effect_apis.contains(&"fs.readFileSync"), "effects: {effect_apis:?}");
    assert!(effect_apis.contains(&"fs.chmodSync"), "effects: {effect_apis:?}");
    assert!(effect_apis.contains(&"fs.writeFileSync"), "awaited sequence import must preserve module identity: {effect_apis:?}");
    assert!(effect_apis.contains(&"fs.unlinkSync"), "nested awaited sequence import must preserve module identity: {effect_apis:?}");
    assert!(effect_apis.contains(&"fs.rmdirSync"), "nested await sequence import must preserve module identity: {effect_apis:?}");
    assert!(effect_apis.contains(&"child_process.exec"), "effects: {effect_apis:?}");
    assert!(!effect_apis.contains(&"fs.symlinkSync"), "non-trailing sequence element must not alias module: {effect_apis:?}");
    assert!(!effect_apis.iter().any(|api| api.contains("watchFile")), "chained sequence import Promise must not alias module: {effect_apis:?}");
    assert!(!effect_apis.contains(&"fs.then"), "unawaited import Promise must not alias module: {effect_apis:?}");
    assert!(!effect_apis.iter().any(|api| api.starts_with("fs.then(")), "chained import Promise must not alias module: {effect_apis:?}");
    assert!(!a.facts.storage.iter().any(|s| s.api.contains("then")), "storage: {:?}", a.facts.storage);

    let ts =
        analyze("main.ts", r#"async function run() { const tsFs = await (0, import(`node\x3afs`) as any); tsFs.renameSync(`/a`, `/b`); }"#, &Limits::default());
    assert!(ts.facts.effects.iter().any(|e| e.api == "fs.renameSync"), "TS type-asserted awaited sequence import: {:?}", ts.facts.effects);

    assert!(a.facts.ipc.iter().any(|i| i.side == "renderer" && i.method == "invoke" && i.channel.as_deref() == Some("note:save")), "ipc: {:?}", a.facts.ipc);
    assert!(a.facts.native_modules.contains(&"bindings:native-addon".to_string()), "native_modules: {:?}", a.facts.native_modules);
}

/// Routes, XHR methods, `contextBridge` keys, `Object.defineProperty(exports, ...)`, and computed CJS exports
/// use cooked template text rather than raw escape spellings.
#[test]
fn arg_str_consumers_decode_escaped_templates_for_routes_xhr_ipc_and_exports() {
    let src = r#"
        app.get(`\x2fapi\x2fnotes`, function h() {});
        const routerConfig = [{ path: `\x2fdashboard` }];
        xhr.open(`P\x4fST`, `/api/submit`);
        contextBridge.exposeInMainWorld(`app\x41pi`, { save() {} });
        Object.defineProperty(exports, `my\x45xport`, { value: 1 });
        Object.defineProperty(exports, `__es\x4dodule`, { value: true });
        exports[`other\x45xport`] = 2;
    "#;

    let a = analyze("preload.cjs", src, &Limits::default());
    assert!(a.facts.routes.iter().any(|r| r.path == "/api/notes" && r.method.as_deref() == Some("GET")), "routes: {:?}", a.facts.routes);
    assert!(a.facts.routes.iter().any(|r| r.path == "/dashboard" && r.source == "router-config"), "routes: {:?}", a.facts.routes);
    assert!(
        a.facts.endpoints.iter().any(|e| e.api == "xhr" && e.method.as_deref() == Some("POST") && e.url == "/api/submit"),
        "endpoints: {:?}",
        a.facts.endpoints
    );
    assert!(
        a.facts.exposed.iter().any(|x| x.key.as_deref() == Some("appApi") && x.members.iter().any(|m| m.function.as_deref() == Some("appApi.save"))),
        "exposed: {:?}",
        a.facts.exposed
    );
    assert!(a.facts.exports.cjs.contains(&"myExport".to_string()), "cjs exports: {:?}", a.facts.exports.cjs);
    assert!(a.facts.exports.cjs.contains(&"otherExport".to_string()), "cjs exports: {:?}", a.facts.exports.cjs);
    assert!(!a.facts.exports.cjs.iter().any(|e| e.contains("esModule") || e.contains("es\\x4dodule")), "cjs exports: {:?}", a.facts.exports.cjs);

    let tsx_src = r#"
        self[`webpackChunk\x61pp`] = [];
        const doc = { kind: `Operation\x44efinition`, operation: `qu\x65ry`, name: { value: `Get\x43ompiled` } };
        const router = createTRPCRouter({ [`by\x49d`]: publicProcedure.query(fn) });
        document.getElementById(`save\x2dbtn`).addEventListener(`cli\x63k`, fn);
        const { BrowserWindow } = require(`electron`);
        new BrowserWindow({ webPreferences: { defaultEncoding: `utf\x2d8`, broken: `\xZZ` } });
        export const view = <Route path={`\x2fsettings`} />;
    "#;
    let tsx = analyze("routes.tsx", tsx_src, &Limits::default());
    assert!(tsx.facts.routes.iter().any(|r| r.path == "/settings" && r.source == "jsx"), "jsx routes: {:?}", tsx.facts.routes);
    assert!(tsx.facts.bundler_markers.contains_key("webpackChunk"), "markers: {:?}", tsx.facts.bundler_markers);
    assert!(tsx.facts.events.iter().any(|ev| ev.target == "document.getElementById(save-btn)" && ev.event == "click"), "events: {:?}", tsx.facts.events);
    assert_eq!(
        tsx.facts.windows[0].web_preferences.get("defaultEncoding"),
        Some(&craft_anything_js::PrefValue::Text("utf-8".into())),
        "windows: {:?}",
        tsx.facts.windows
    );
    assert_eq!(
        tsx.facts.windows[0].web_preferences.get("broken"),
        Some(&craft_anything_js::PrefValue::Text("dynamic".into())),
        "windows: {:?}",
        tsx.facts.windows
    );
    assert!(
        tsx.facts.api_operations.iter().any(|op| op.protocol == "graphql" && op.kind == "query" && op.name.as_deref() == Some("GetCompiled")),
        "api ops: {:?}",
        tsx.facts.api_operations
    );
    assert!(
        tsx.facts.api_operations.iter().any(|op| op.protocol == "trpc" && op.kind == "query" && op.name.as_deref() == Some("byId")),
        "api ops: {:?}",
        tsx.facts.api_operations
    );
}

/// Webpack chunk IDs, webpack module-loader calls, and browserify dependency maps accept no-substitution template literals.
#[test]
fn bundle_loaders_and_chunk_ids_accept_cooked_template_literals() {
    let webpack_src = r#"
        (self.webpackChunkapp = self.webpackChunkapp || []).push([
            [`chunk\x2dmain`],
            {
                "./src/index.js": function(m, e, req) {
                    const dep = req(`./dep\x2ejs`);
                    m.exports = dep;
                },
                "./dep.js": function(m) {
                    m.exports = 1;
                }
            }
        ]);
    "#;
    let wp = analyze("chunk.js", webpack_src, &Limits::default());
    assert_eq!(wp.bundle.chunk_ids, vec!["chunk-main"]);
    let entry = wp.bundle.modules.iter().find(|m| m.id == "./src/index.js").expect("entry module");
    assert_eq!(entry.deps, vec!["./dep.js"]);

    let browserify_src = r#"
        (function e(t,n,r){})({
            "1": [function(require, module, exports) { require("./dep"); }, { "./dep": `\x32` }],
            "2": [function(require, module, exports) { module.exports = 42; }, {}]
        },{},[1]);
    "#;
    let bf = analyze("bf.js", browserify_src, &Limits::default());
    let m1 = bf.bundle.modules.iter().find(|m| m.id == "1").expect("browserify module 1");
    assert_eq!(m1.deps, vec!["2"]);
}

/// Untagged template literals containing invalid escapes (`cooked: None`) are not reported as literal raw strings.
#[test]
fn invalid_escape_untagged_templates_are_not_reported_as_raw_literals() {
    let src = r#"
        require(`./bad\8name`);
        fetch(`https://ex.test/\xZZ`);
        const u = `https://ex.test/graphql\xZZ/${id}`;
        const q = `query GetBad\xZZ { a } ${frag}`;
        gql(`query GetCallBad\xZZ { a }`);
    "#;

    let a = analyze("broken.js", src, &Limits::default());
    assert!(
        !a.facts.imports.iter().any(|i| i.literal && i.specifier.contains("bad")),
        "invalid template must not produce literal import: {:?}",
        a.facts.imports
    );
    assert!(
        !a.facts.endpoints.iter().any(|e| e.literal && e.url.contains("ex.test")),
        "invalid template must not produce literal endpoint: {:?}",
        a.facts.endpoints
    );
    assert!(
        !a.facts.strings.iter().any(|s| s.contains("ex.test") || s.contains("bad")),
        "invalid template must not enter literal strings: {:?}",
        a.facts.strings
    );
    assert!(
        !a.facts
            .api_operations
            .iter()
            .any(|op| matches!(op.name.as_deref(), Some("GetBad" | "GetCallBad")) || op.url.as_deref().is_some_and(|u| u.contains("ex.test"))),
        "invalid untagged template must not produce named/url api operations: {:?}",
        a.facts.api_operations
    );
}

/// Ordinary string literals, interpolated templates, and ES2018 tagged templates (without triggering untagged `webpackChunk` hints) retain existing behavior.
#[test]
fn tagged_gql_and_plain_string_literals_control() {
    let src = "
        const fs = require(\"node:fs\");
        fetch(`/api/items/${itemId}`);
        const DOC = gql`query GetItems { items { id } }`;
        const RAW_DOC = gql`# raw escape \\xZZ allowed in ES2018 tagged template
query GetRawItems { items { id } }`;
        const rawChunk = String.raw`webpack\\x43hunkapp`;
        const queue = [];
        queue.push([[1], { callback: () => 42 }]);
    ";

    let a = analyze("control.js", src, &Limits::default());
    assert!(a.facts.imports.iter().any(|i| i.specifier == "node:fs" && i.literal));
    assert!(a.facts.endpoints.iter().any(|e| e.api == "fetch" && e.url == "/api/items/{itemId}" && !e.literal));
    assert!(a.facts.api_operations.iter().any(|op| op.protocol == "graphql" && op.name.as_deref() == Some("GetItems")));
    assert!(a.facts.api_operations.iter().any(|op| op.protocol == "graphql" && op.name.as_deref() == Some("GetRawItems")), "ops: {:?}", a.facts.api_operations);
    assert!(!a.facts.bundler_markers.contains_key("webpackChunk"), "tagged template must not set webpackChunk marker: {:?}", a.facts.bundler_markers);
    assert!(a.bundle.modules.is_empty(), "ordinary queue.push must not become webpack-chunk module: {:?}", a.bundle.modules);
}

/// Nested comma sequences inside bare `await` initializers resolve module aliases through depth 23 and stop at `MAX_PATH_DEPTH` (24).
#[test]
fn awaited_sequence_depth_cap_stops_at_max_path_depth() {
    let wrap = |depth: usize| format!("{}import(`node\\x3afs`){}", "(0, ".repeat(depth), ")".repeat(depth));
    let src = format!(
        "async function run() {{\n  const fs23 = await {s23};\n  fs23.accessSync(`/tmp/ok`);\n  const fs24 = await {s24};\n  fs24.statSync(`/tmp/over`);\n}}",
        s23 = wrap(23),
        s24 = wrap(24),
    );
    let a = analyze("depth.js", &src, &Limits::default());
    let effect_apis: Vec<&str> = a.facts.effects.iter().map(|e| e.api.as_str()).collect();
    assert!(effect_apis.contains(&"fs.accessSync"), "23 nested sequences must resolve: {effect_apis:?}");
    assert!(!effect_apis.contains(&"fs.statSync"), "24 nested sequences must stop at MAX_PATH_DEPTH: {effect_apis:?}");
}
