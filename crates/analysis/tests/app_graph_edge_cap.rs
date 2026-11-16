//! An application graph records the edges its cap drops, and a graph cut by either cap is not
//! reported as complete.

use craft_anything_analysis::js_app::{AppFile, AppInput, AppOptions, analyze_application};

fn mutually_importing(n: usize) -> Vec<AppFile> {
    (0..n)
        .map(|i| {
            let src: String = (0..n).filter(|j| *j != i).map(|j| format!("require('./m{j}.js');\n")).collect();
            AppFile { path: format!("m{i}.js"), bytes: src.into_bytes() }
        })
        .collect()
}

fn graph_of(files: Vec<AppFile>, max_nodes: usize) -> craft_anything_analysis::js_app::AppGraph {
    let listing = files.iter().map(|f| (f.path.clone(), f.bytes.len() as u64)).collect();
    analyze_application(&AppInput { files, listing, ..Default::default() }, &AppOptions { max_nodes, ..Default::default() }, &|| false).unwrap()
}

#[test]
fn dropped_edges_are_counted_and_reported() {
    let graph = graph_of(mutually_importing(16), 32);
    assert_eq!(graph.edges.len(), 32 * 4, "the edge cap is 4 x max_nodes");
    assert!(graph.limitations.iter().any(|(code, message)| code == "edge_cap" && message.contains("dropped")), "{:?}", graph.limitations);
}

#[test]
fn a_graph_within_the_edge_cap_reports_no_cap() {
    let graph = graph_of(mutually_importing(4), 100);
    assert_eq!(graph.edges.len(), 4 * 3);
    assert!(!graph.limitations.iter().any(|(code, _)| code == "edge_cap" || code == "node_cap"), "{:?}", graph.limitations);
}
