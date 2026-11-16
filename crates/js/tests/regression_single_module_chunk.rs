//! A webpack chunk pushes `[[chunk ids], {module id: factory}]` onto a global array. A push whose
//! object had exactly one property was taken for a one-module chunk whatever it held, so ordinary
//! code such as `queue.push([[1], { callback: () => 42 }])` made a webpack module and named the
//! file a webpack bundle.

use craft_anything_js::{Analysis, Limits, ParseMode, analyze};

fn parsed(src: &str) -> Analysis {
    let analysis = analyze("app.js", src, &Limits::default());
    assert_eq!(analysis.facts.mode, ParseMode::Parsed, "{:?}", analysis.facts.mode_reason);
    analysis
}

fn modules(a: &Analysis) -> Vec<(String, String)> {
    a.bundle.modules.iter().map(|m| (m.id.clone(), m.format.clone())).collect()
}

#[test]
fn ordinary_pushes_are_not_chunks() {
    for src in [
        "const queue = [];\nqueue.push([[1], { callback: () => 42 }]);\n",
        "const queue = [];\nqueue.push([[1], { callback(job) { return job; } }]);\n",
        "const rows = [];\nrows.push([[1, 2], { value: 42 }]);\n",
    ] {
        let a = parsed(src);
        assert!(a.bundle.modules.is_empty(), "{src}: {:?}", modules(&a));
        assert!(a.bundle.chunk_ids.is_empty(), "{src}: {:?}", a.bundle.chunk_ids);
        assert!(a.bundle.bundlers.is_empty(), "{src}: {:?}", a.bundle.bundlers);
    }
}

/// webpack's own one-module chunks still read, with the default global, a renamed one
/// (`output.chunkLoadingGlobal`) or `||=`, and with webpack named elsewhere in the file.
#[test]
fn one_module_chunks_still_read() {
    for src in [
        "(self.webpackChunkapp=self.webpackChunkapp||[]).push([[179],{512:function(e,t,n){e.exports=1}}]);",
        "(window.webpackJsonp=window.webpackJsonp||[]).push([[179],{512:function(e,t,n){e.exports=1}}]);",
        "(self.myChunks=self.myChunks||[]).push([[179],{\"./src/a.js\":(e,t,n)=>{e.exports=1}}]);",
        "(globalThis.myChunks||=[]).push([[179],{512:()=>{}}]);",
        "var q = window.queue;\nq.push([[179],{512:function(e){e.exports=1}}]);\n__webpack_require__.p = '/';",
    ] {
        let a = parsed(src);
        assert_eq!(a.bundle.modules.len(), 1, "{src}: {:?}", modules(&a));
        assert_eq!(a.bundle.modules[0].format, "webpack-chunk", "{src}");
        assert_eq!(a.bundle.chunk_ids, ["179"], "{src}");
        assert!(a.bundle.bundlers.iter().any(|b| b == "webpack"), "{src}: {:?}", a.bundle.bundlers);
    }
}

/// A one-property object that is not a factory is not a module, even in a webpack chunk.
#[test]
fn a_single_value_that_is_not_a_factory() {
    let a = parsed("(self.webpackChunkapp=self.webpackChunkapp||[]).push([[179],{512:42}]);");
    assert!(a.bundle.modules.is_empty(), "{:?}", modules(&a));
    assert!(a.bundle.chunk_ids.is_empty(), "{:?}", a.bundle.chunk_ids);
}
