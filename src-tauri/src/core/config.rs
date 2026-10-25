//! Tunable constants and scan options. Values are starting points to be
//! tuned against real disks (see sub-task 8).

/// Files smaller than this are folded into their directory's aggregate
/// counters instead of getting their own node.
pub const DEFAULT_SMALL_FILE_THRESHOLD_BYTES: u64 = 1024 * 1024;

/// Number of largest children returned per directory by the view query.
pub const DEFAULT_TOP_K: usize = 64;

/// Levels of children included below the requested node.
pub const DEFAULT_VIEW_DEPTH: usize = 2;

/// Hard cap on nodes in one view. Depth and top-K alone allow (K+1)^depth
/// nodes, so this is what actually bounds the payload.
pub const MAX_VIEW_NODES: usize = 5_000;

/// Upper limits applied to values requested over IPC.
pub const MAX_VIEW_DEPTH: usize = 6;
pub const MAX_TOP_K: usize = 256;

/// Files smaller than this are not looked at by the duplicate finder.
pub const DEFAULT_DUPLICATE_MIN_SIZE_BYTES: u64 = 1024 * 1024;

/// Smallest minimum size the duplicate finder accepts over IPC.
pub const MIN_DUPLICATE_MIN_SIZE_BYTES: u64 = 4 * 1024;

/// Most groups returned in one page of duplicate results.
pub const MAX_DUPLICATE_GROUPS_PER_PAGE: usize = 200;

/// Most scan problems sent to the UI. The full count is sent as well.
pub const MAX_DUPLICATE_ISSUES_SHOWN: usize = 100;

/// Most files one duplicate cleanup plan may move. Same limit as the other
/// cleanup plans.
pub const MAX_DUPLICATE_MOVES_PER_PLAN: usize = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOptions {
    pub small_file_threshold: u64,
    pub cross_volumes: bool,
    /// Cloud-sync folders (File Provider mounts, iCloud Drive) can take
    /// minutes to list, so they are skipped unless this is set.
    pub include_cloud_folders: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            small_file_threshold: DEFAULT_SMALL_FILE_THRESHOLD_BYTES,
            cross_volumes: false,
            include_cloud_folders: false,
        }
    }
}
