//! View commands: bounded subtree queries over the scanned tree.
//! The UI sends a node id and limits; it never receives the full tree.

use std::path::Path;

use serde::{Deserialize, Serialize};
use tauri::State;

use super::scan::ScanState;
use crate::core::config::{MAX_TOP_K, MAX_VIEW_DEPTH, MAX_VIEW_NODES};
use crate::core::tree::{NodeId, Tree};
use crate::core::view::{Crumb, ViewNode, ViewOptions, breadcrumbs, build_view, find_by_path};

#[derive(Deserialize, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ViewRequest {
    /// Raw node id; the scan root when omitted.
    pub node: Option<u32>,
    pub depth: Option<usize>,
    pub top_k: Option<usize>,
}

#[derive(Serialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ViewResponse {
    pub view: ViewNode,
    pub breadcrumbs: Vec<Crumb>,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ViewErrorKind {
    NoScan,
    UnknownNode,
}

#[derive(Serialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ViewError {
    kind: ViewErrorKind,
    message: String,
}

impl ViewError {
    fn no_scan() -> Self {
        Self {
            kind: ViewErrorKind::NoScan,
            message: "No scan has completed yet".into(),
        }
    }
}

/// Applies defaults and clamps values that came over IPC.
fn resolve_options(request: &ViewRequest) -> ViewOptions {
    let defaults = ViewOptions::default();
    ViewOptions {
        depth: request.depth.unwrap_or(defaults.depth).min(MAX_VIEW_DEPTH),
        top_k: request.top_k.unwrap_or(defaults.top_k).min(MAX_TOP_K),
        max_nodes: MAX_VIEW_NODES,
    }
}

pub fn query_view(tree: &Tree, request: &ViewRequest) -> Result<ViewResponse, ViewError> {
    let node = match request.node {
        None => NodeId::ROOT,
        Some(raw) => tree.node_id(raw).ok_or_else(|| ViewError {
            kind: ViewErrorKind::UnknownNode,
            message: format!("Node {raw} does not exist in the current scan"),
        })?,
    };
    Ok(ViewResponse {
        view: build_view(tree, node, &resolve_options(request)),
        breadcrumbs: breadcrumbs(tree, node),
    })
}

/// Raw id of the node at `path`, or `None` when it is not in the tree.
pub fn lookup_path(tree: &Tree, path: &str) -> Option<u32> {
    find_by_path(tree, Path::new(path)).map(NodeId::raw)
}

fn with_tree<T>(
    state: &ScanState,
    f: impl FnOnce(&Tree) -> Result<T, ViewError>,
) -> Result<T, ViewError> {
    let guard = state.tree.lock().expect("scan state poisoned");
    f(guard.as_ref().ok_or_else(ViewError::no_scan)?)
}

#[tauri::command]
pub fn get_view(
    state: State<'_, ScanState>,
    request: ViewRequest,
) -> Result<ViewResponse, ViewError> {
    with_tree(&state, |tree| query_view(tree, &request))
}

#[tauri::command]
pub fn find_node_by_path(
    state: State<'_, ScanState>,
    path: String,
) -> Result<Option<u32>, ViewError> {
    with_tree(&state, |tree| Ok(lookup_path(tree, &path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;

    fn tree() -> Tree {
        let mut tree = Tree::new("/root");
        let dir = tree.add_dir(NodeId::ROOT, "dir");
        tree.add_file(dir, "f", MIB);
        tree.rollup();
        tree
    }

    #[test]
    fn defaults_to_the_root() {
        let response = query_view(&tree(), &ViewRequest::default()).unwrap();
        assert_eq!(response.view.name, "/root");
        assert_eq!(response.breadcrumbs.len(), 1);
    }

    #[test]
    fn unknown_node_is_a_typed_error() {
        let request = ViewRequest {
            node: Some(9999),
            ..ViewRequest::default()
        };
        let error = query_view(&tree(), &request).unwrap_err();
        assert_eq!(error.kind, ViewErrorKind::UnknownNode);
    }

    #[test]
    fn requested_limits_are_clamped() {
        let options = resolve_options(&ViewRequest {
            node: None,
            depth: Some(usize::MAX),
            top_k: Some(usize::MAX),
        });
        assert_eq!(options.depth, MAX_VIEW_DEPTH);
        assert_eq!(options.top_k, MAX_TOP_K);
        assert_eq!(options.max_nodes, MAX_VIEW_NODES);
    }

    #[test]
    fn missing_limits_use_defaults() {
        let options = resolve_options(&ViewRequest::default());
        assert_eq!(options, ViewOptions::default());
    }

    #[test]
    fn request_and_response_use_camel_case() {
        let request: ViewRequest =
            serde_json::from_str(r#"{"node":3,"topK":5,"depth":1}"#).unwrap();
        assert_eq!(
            (request.node, request.top_k, request.depth),
            (Some(3), Some(5), Some(1))
        );
        let json =
            serde_json::to_value(query_view(&tree(), &ViewRequest::default()).unwrap()).unwrap();
        assert!(json.get("breadcrumbs").is_some());
        assert_eq!(
            serde_json::to_value(ViewErrorKind::NoScan).unwrap(),
            "noScan"
        );
    }

    #[test]
    fn path_lookup_returns_raw_ids() {
        let tree = tree();
        let id = lookup_path(&tree, "/root/dir").unwrap();
        assert_eq!(tree.name(tree.node_id(id).unwrap()), "dir");
        assert_eq!(lookup_path(&tree, "/root/missing"), None);
    }

    #[test]
    fn no_scan_error_when_state_is_empty() {
        let state = ScanState::default();
        let error = with_tree(&state, |_| Ok(())).unwrap_err();
        assert_eq!(error.kind, ViewErrorKind::NoScan);
    }

    /// Manual benchmark. Run with:
    /// `CTD_BENCH_ROOT=/path cargo test --release bench_view -- --ignored --nocapture`
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn bench_view() {
        use crate::core::config::ScanOptions;
        use crate::core::scan::CancelToken;
        use crate::platform::Scanner;
        use std::time::Instant;

        let root = std::env::var("CTD_BENCH_ROOT").expect("set CTD_BENCH_ROOT");
        let scanned = crate::platform::macos::MacScanner::new()
            .scan(
                Path::new(&root),
                &ScanOptions::default(),
                &CancelToken::default(),
                &|_| {},
            )
            .unwrap();
        let tree = scanned.tree;
        println!(
            "tree nodes={} total_bytes={}",
            tree.len(),
            tree.size(NodeId::ROOT)
        );

        let report = |label: &str, request: ViewRequest| {
            let start = Instant::now();
            let response = query_view(&tree, &request).unwrap();
            let build = start.elapsed();
            let start = Instant::now();
            let json = serde_json::to_string(&response).unwrap();
            let encode = start.elapsed();
            fn count(n: &ViewNode) -> usize {
                1 + n.children.iter().map(count).sum::<usize>()
            }
            println!(
                "{label:<28} nodes={:>5} json={:>8} B build={build:>9.2?} encode={encode:>9.2?}",
                count(&response.view),
                json.len()
            );
        };
        let req = |depth, top_k| ViewRequest {
            node: None,
            depth: Some(depth),
            top_k: Some(top_k),
        };
        report("default (depth 2, k 64)", ViewRequest::default());
        report("depth 1, k 64", req(1, 64));
        report("depth 3, k 64", req(3, 64));
        report("depth 6, k 256 (max)", req(6, 256));
    }
}
