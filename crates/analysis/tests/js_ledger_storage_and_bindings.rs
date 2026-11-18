//! Regression tests for `js_ledger`: valid `source_node` links across `storage`, `endpoints`, and
//! `native_modules`, effective-key deduplication in `candidate_expectations`, and order-independent
//! aggregation of multiple `Binding` records from the same owner in `build_ledger`.

use craft_anything_analysis::js_app::{AppFile, AppGraph, AppInput, AppOptions, Node, analyze_application};
use craft_anything_analysis::js_ledger::{Binding, Case, Owner, Review, Verifier, build_ledger, candidate_expectations};
use serde_json::{Value, json};
use std::collections::BTreeSet;

/// Build an `AppGraph` from synthetic files and extra listed paths.
fn make_graph(files: &[(&str, &str)], extra_listed: &[&str]) -> AppGraph {
    let app_files: Vec<AppFile> = files.iter().map(|(p, c)| AppFile { path: (*p).to_string(), bytes: c.as_bytes().to_vec() }).collect();
    let mut listing: Vec<(String, u64)> = app_files.iter().map(|f| (f.path.clone(), f.bytes.len() as u64)).collect();
    for p in extra_listed {
        listing.push(((*p).to_string(), 128));
    }
    let input = AppInput { label: "ledger-app".into(), format: "directory".into(), files: app_files, listing, notes: Vec::new() };
    analyze_application(&input, &AppOptions::default(), &|| false).expect("analysis should succeed")
}

/// Build a representative Electron + web graph exercising every expectation layer.
fn rich_app_graph() -> AppGraph {
    make_graph(
        &[
            ("package.json", r#"{"name":"ledger-app","main":"main.js","devDependencies":{"electron":"^33.0.0"}}"#),
            (
                "main.js",
                concat!(
                    r#"const { BrowserWindow, ipcMain } = require("electron"); const fs = require("fs"); const cp = require("child_process");"#,
                    "\n",
                    r#"const w1 = new BrowserWindow({ webPreferences: { preload: "preload1.js" } }); const w2 = new BrowserWindow({ webPreferences: { preload: "preload2.js" } });"#,
                    "\n",
                    r#"w1.loadFile("index.html");"#,
                    "\n",
                    r#"ipcMain.handle("notes:save", () => true);"#,
                    "\n",
                    r#"fs.writeFileSync("notes.json", "{}");"#,
                    "\n",
                    r#"cp.exec("git status");"#,
                    "\n",
                ),
            ),
            (
                "preload1.js",
                r#"
                const { contextBridge, ipcRenderer } = require("electron");
                contextBridge.exposeInMainWorld("api", { save: (x) => ipcRenderer.invoke("notes:save", x) });
                "#,
            ),
            (
                "preload2.js",
                r#"
                const { contextBridge, ipcRenderer } = require("electron");
                contextBridge.exposeInMainWorld("api", { save: (x) => ipcRenderer.invoke("notes:save", x) });
                "#,
            ),
            ("index.html", r#"<!doctype html><script src="./renderer.js"></script>"#),
            (
                "renderer.js",
                r#"
                localStorage.setItem("theme", "dark");
                localStorage.clear();
                fetch("https://api.example.test/v1/notes");
                fetch("https://api.example.test/v1/notes", { method: "POST" });
                app.get("/health", () => {});
                require("./direct.node");
                "#,
            ),
        ],
        &["build/Release/pty.node"],
    )
}

/// Every candidate expectation references an existing `AppGraph` `source_node` and carries a unique `expectation_id`.
#[test]
fn every_candidate_expectation_source_node_resolves_and_ids_are_unique() {
    let g = rich_app_graph();
    let exps = candidate_expectations(&g);
    assert!(!exps.is_empty());

    let mut ids = BTreeSet::new();
    let mut layers = BTreeSet::new();
    for exp in &exps {
        assert!(
            g.node(&exp.source_node).is_some(),
            "expectation {:?} (target {:?}, layer {:?}) points to missing source_node {:?}",
            exp.expectation_id,
            exp.target,
            exp.layer,
            exp.source_node
        );
        assert!(ids.insert(exp.expectation_id.clone()), "duplicate expectation_id {} for target {}", exp.expectation_id, exp.target);
        layers.insert(exp.layer.as_str());
    }

    for expected_layer in ["install", "shell", "network", "page", "storage", "system", "native"] {
        assert!(layers.contains(expected_layer), "missing layer {expected_layer}: {layers:?}");
    }
}

/// `fs.*` effects remapped into the `storage` layer deduplicate against matching `storage` targets by effective `(layer, target)`.
#[test]
fn remapped_fs_effects_and_storage_entries_deduplicate_by_effective_expectation_id() {
    let mut g = rich_app_graph();
    g.storage.push(json!({
        "api": "fs.writeFileSync",
        "arg": "notes.json",
        "module": "main.js",
        "line": 5,
        "function": null,
        "role": "main"
    }));
    g.nodes.push(Node {
        id: "storage:fs.writeFileSync notes.json".into(),
        kind: "storage".into(),
        label: "fs.writeFileSync notes.json".into(),
        module: None,
        line: None,
        role: None,
        attrs: Value::Null,
    });
    let is_notes_write = |u: &Value| u["api"] == "fs.writeFileSync" && u["arg"] == "notes.json";
    assert!(g.effects.iter().any(is_notes_write), "the fixture's fs effect is missing: {:?}", g.effects);
    assert!(g.storage.iter().any(is_notes_write), "the storage entry is missing: {:?}", g.storage);

    let exps = candidate_expectations(&g);
    let ids: BTreeSet<&str> = exps.iter().map(|e| e.expectation_id.as_str()).collect();
    assert_eq!(ids.len(), exps.len(), "effective (layer, target) collision produced duplicate expectation_ids: {exps:?}");
    let notes: Vec<_> = exps.iter().filter(|e| e.target == "effect:fs.writeFileSync notes.json").collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0].layer, "storage");
    assert!(g.node(&notes[0].source_node).is_some(), "source_node {:?} does not resolve", notes[0].source_node);

    let ledger = build_ledger(&g, &[], &[], &|_| true);
    let rows = ledger["expectations"].as_array().expect("expectations array");
    assert_eq!(rows.len(), exps.len());
}

/// Multiple `Binding` entries from the same owner merge their `cases`, `parser_type`, and `verifier` in either order, while blank `module_path` stays `described`.
#[test]
fn same_owner_split_bindings_merge_cases_parser_type_and_verifier_in_either_order() {
    let g = rich_app_graph();
    let ipc = candidate_expectations(&g).into_iter().find(|e| e.target == "ipc:notes:save").expect("ipc:notes:save expectation");
    let ok = |id: &str| id.starts_with("ev_ok");

    let part_a = Binding {
        expectation_id: ipc.expectation_id.clone(),
        owner: Owner { module_path: "src/ipc.rs".into(), symbol: "save_notes".into(), impl_sha256: None },
        parser_type: Some("SaveNotesReq".into()),
        cases: vec![
            Case { kind: "works".into(), evidence_id: "ev_ok_1".into(), strength: "module-test".into() },
            Case { kind: "refuses".into(), evidence_id: "ev_ok_2".into(), strength: "module-test".into() },
        ],
        verifier: None,
    };
    let part_b = Binding {
        expectation_id: ipc.expectation_id.clone(),
        owner: Owner { module_path: "  src/ipc.rs  ".into(), symbol: " save_notes ".into(), impl_sha256: None },
        parser_type: None,
        cases: vec![Case { kind: "bad-input".into(), evidence_id: "ev_ok_3".into(), strength: "module-test".into() }],
        verifier: Some(Verifier {
            command: "cargo test".into(),
            strength: "module-test".into(),
            status: "pass".into(),
            result_evidence: "ev_ok_ver".into(),
            expectation_ids: vec![ipc.expectation_id.clone()],
            nondeterminism: None,
        }),
    };

    let status_of = |l: &Value| l["expectations"].as_array().unwrap().iter().find(|r| r["expectation_id"] == ipc.expectation_id).unwrap().clone();

    let fwd = build_ledger(&g, &[part_a.clone(), part_b.clone()], &[], &ok);
    let rev = build_ledger(&g, &[part_b, part_a], &[], &ok);
    assert_eq!(status_of(&fwd)["status"], "proven", "forward row: {}", status_of(&fwd));
    assert_eq!(status_of(&rev)["status"], "proven", "reverse row: {}", status_of(&rev));
    assert_eq!(fwd["ledger_id"], rev["ledger_id"]);
    assert_eq!(fwd["closed_hash"], rev["closed_hash"]);

    let blank_mod = Binding {
        expectation_id: ipc.expectation_id.clone(),
        owner: Owner { module_path: "   ".into(), symbol: "save_notes".into(), impl_sha256: None },
        parser_type: Some("SaveNotesReq".into()),
        cases: vec![
            Case { kind: "works".into(), evidence_id: "ev_ok_1".into(), strength: "module-test".into() },
            Case { kind: "refuses".into(), evidence_id: "ev_ok_2".into(), strength: "module-test".into() },
            Case { kind: "bad-input".into(), evidence_id: "ev_ok_3".into(), strength: "module-test".into() },
        ],
        verifier: None,
    };
    let blank_res = status_of(&build_ledger(&g, &[blank_mod], &[], &ok));
    assert_eq!(blank_res["status"], "described", "blank module_path must not become built: {blank_res}");
}

/// Same-owner verifiers never hide a `fail` behind an earlier `pass`, upgrade a weak `pass` when a strong `pass` is also bound, and preserve `unclear` across orders.
#[test]
fn same_owner_failing_or_stronger_verifiers_are_order_independent_and_never_hidden() {
    let g = rich_app_graph();
    let ipc = candidate_expectations(&g).into_iter().find(|e| e.target == "ipc:notes:save").expect("ipc:notes:save");
    let ok = |id: &str| id.starts_with("ev_ok");

    let mk = |strength: &str, status: &str, ev: &str| Binding {
        expectation_id: ipc.expectation_id.clone(),
        owner: Owner { module_path: "src/ipc.rs".into(), symbol: "save_notes".into(), impl_sha256: None },
        parser_type: Some("SaveNotesReq".into()),
        cases: vec![
            Case { kind: "works".into(), evidence_id: "ev_ok_1".into(), strength: "module-test".into() },
            Case { kind: "refuses".into(), evidence_id: "ev_ok_2".into(), strength: "module-test".into() },
            Case { kind: "bad-input".into(), evidence_id: "ev_ok_3".into(), strength: "module-test".into() },
        ],
        verifier: Some(Verifier {
            command: "cargo test".into(),
            strength: strength.into(),
            status: status.into(),
            result_evidence: ev.into(),
            expectation_ids: vec![ipc.expectation_id.clone()],
            nondeterminism: None,
        }),
    };

    let row_status = |l: &Value| l["expectations"].as_array().unwrap().iter().find(|r| r["expectation_id"] == ipc.expectation_id).unwrap()["status"].clone();

    let pass_b = mk("module-test", "pass", "ev_ok_a");
    let fail_b = mk("module-test", "fail", "ev_ok_b");
    let pf = build_ledger(&g, &[pass_b.clone(), fail_b.clone()], &[], &ok);
    let fp = build_ledger(&g, &[fail_b, pass_b], &[], &ok);
    assert_eq!(row_status(&pf), "disproved");
    assert_eq!(row_status(&fp), "disproved");
    assert_eq!(pf["status"], "failing");
    assert_eq!(fp["status"], "failing");

    let weak_b = mk("unit-test", "pass", "ev_ok_weak");
    let strong_b = mk("module-test", "pass", "ev_ok_strong");
    let ws = build_ledger(&g, &[weak_b.clone(), strong_b.clone()], &[], &ok);
    let sw = build_ledger(&g, &[strong_b, weak_b.clone()], &[], &ok);
    assert_eq!(row_status(&ws), "proven");
    assert_eq!(row_status(&sw), "proven");
    assert_eq!(ws["closed_hash"], sw["closed_hash"]);

    let missing_strong_b = mk("module-test", "pass", "ev_missing_tampered");
    let wm = build_ledger(&g, &[weak_b.clone(), missing_strong_b.clone()], &[], &ok);
    let mw = build_ledger(&g, &[missing_strong_b, weak_b], &[], &ok);
    assert_eq!(row_status(&wm), "unclear");
    assert_eq!(row_status(&mw), "unclear");
}

/// Distinct conflicting owners, single-binding proofs, and `skipped` reviews retain existing behavior.
#[test]
fn conflicting_distinct_owners_and_single_binding_controls() {
    let g = rich_app_graph();
    let ipc = candidate_expectations(&g).into_iter().find(|e| e.target == "ipc:notes:save").expect("ipc:notes:save");
    let ok = |id: &str| id.starts_with("ev_ok");

    let b1 = Binding {
        expectation_id: ipc.expectation_id.clone(),
        owner: Owner { module_path: "src/a.rs".into(), symbol: "save_a".into(), impl_sha256: None },
        parser_type: Some("Req".into()),
        cases: Vec::new(),
        verifier: None,
    };
    let b2 = Binding {
        expectation_id: ipc.expectation_id.clone(),
        owner: Owner { module_path: "src/b.rs".into(), symbol: "save_b".into(), impl_sha256: None },
        parser_type: Some("Req".into()),
        cases: Vec::new(),
        verifier: None,
    };
    let conflict = build_ledger(&g, &[b1, b2], &[], &ok);
    assert_eq!(conflict["status"], "failing");

    let skipped = build_ledger(&g, &[], &[Review { expectation_id: ipc.expectation_id.clone(), decision: "skipped".into(), reason: None }], &ok);
    let row = skipped["expectations"].as_array().unwrap().iter().find(|r| r["expectation_id"] == ipc.expectation_id).unwrap();
    assert_eq!(row["status"], "skipped");
}

/// Two passing verifiers that tie on command and result but differ in strength report the same gap in either order.
#[test]
fn tied_verifiers_report_the_same_gap_in_either_order() {
    let g = rich_app_graph();
    let entry = candidate_expectations(&g).into_iter().find(|e| e.min_strength == "built-app").expect("a built-app expectation");
    let ok = |id: &str| id.starts_with("ev_ok");
    let mk = |strength: &str| Binding {
        expectation_id: entry.expectation_id.clone(),
        owner: Owner { module_path: "src/main.rs".into(), symbol: "main".into(), impl_sha256: None },
        parser_type: Some("Args".into()),
        cases: entry.must_cover.iter().map(|k| Case { kind: k.clone(), evidence_id: "ev_ok_case".into(), strength: "built-app".into() }).collect(),
        verifier: Some(Verifier {
            command: "cargo test".into(),
            strength: strength.into(),
            status: "pass".into(),
            result_evidence: "ev_ok_result".into(),
            expectation_ids: vec![entry.expectation_id.clone()],
            nondeterminism: None,
        }),
    };
    let row = |l: &Value| l["expectations"].as_array().unwrap().iter().find(|r| r["expectation_id"] == entry.expectation_id).unwrap().clone();

    let (unit, module) = (mk("unit-test"), mk("module-test"));
    let fwd = row(&build_ledger(&g, &[unit.clone(), module.clone()], &[], &ok));
    let rev = row(&build_ledger(&g, &[module, unit], &[], &ok));
    for r in [&fwd, &rev] {
        assert_eq!(r["status"], "built", "{r}");
        let missing = r["missing"].as_array().expect("missing list");
        assert_eq!(missing.len(), 1, "the verifier strength is the only gap: {r}");
        assert!(missing[0].as_str().is_some_and(|m| m.starts_with("check strength")), "{r}");
    }
    assert_eq!(fwd["missing"], rev["missing"], "forward {fwd} / reverse {rev}");
}

/// A verifier result that only starts to verify partway through one build doesn't prove the row in that build.
#[test]
fn a_result_published_during_the_build_does_not_prove_the_row() {
    let g = rich_app_graph();
    let ipc = candidate_expectations(&g).into_iter().find(|e| e.target == "ipc:notes:save").expect("ipc:notes:save");
    let late_checks = std::cell::Cell::new(0);
    let ok = |id: &str| {
        if id == "ev_late" {
            late_checks.set(late_checks.get() + 1);
            return late_checks.get() > 1;
        }
        id.starts_with("ev_ok")
    };
    let b = Binding {
        expectation_id: ipc.expectation_id.clone(),
        owner: Owner { module_path: "src/ipc.rs".into(), symbol: "save_notes".into(), impl_sha256: None },
        parser_type: Some("SaveNotesReq".into()),
        cases: ipc.must_cover.iter().map(|k| Case { kind: k.clone(), evidence_id: "ev_ok_case".into(), strength: "module-test".into() }).collect(),
        verifier: Some(Verifier {
            command: "cargo test".into(),
            strength: "module-test".into(),
            status: "pass".into(),
            result_evidence: "ev_late".into(),
            expectation_ids: vec![ipc.expectation_id.clone()],
            nondeterminism: None,
        }),
    };
    let row = |l: &Value| l["expectations"].as_array().unwrap().iter().find(|r| r["expectation_id"] == ipc.expectation_id).unwrap().clone();
    let first = build_ledger(&g, std::slice::from_ref(&b), &[], &ok);
    assert_eq!(row(&first)["status"], "unclear", "{}", row(&first));
    assert_eq!(late_checks.get(), 1, "the result was checked more than once in one build");
    assert_ne!(first["status"], "ready");

    // The next build checks again and accepts it: nothing is remembered across builds.
    let next = build_ledger(&g, &[b], &[], &ok);
    assert_eq!(row(&next)["status"], "proven", "{}", row(&next));
    assert_eq!(late_checks.get(), 2);
}
