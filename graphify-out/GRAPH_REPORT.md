# Graph Report - carbon-treemap-disk  (2026-10-25)

## Corpus Check
- Corpus is ~46,972 words - fits in a single context window. You may not need a graph.

## Summary
- 1149 nodes · 2805 edges · 51 communities (39 shown, 12 thin omitted)
- Extraction: 99% EXTRACTED · 1% INFERRED · 0% AMBIGUOUS · INFERRED: 35 edges (avg confidence: 0.85)
- Token cost: 1,200 input · 450 output

## Community Hubs (Navigation)
- Core Cleanup & Guardrails
- Treemap View & Breadcrumbs
- Folder Table & Formatting UI
- Scan Engine & Thread Channels
- Cleanup Execution Logging
- Arena Tree Storage & Rollup
- Cleanup IPC & Plan Commands
- Cleanup Scan State & Errors
- Scan IPC & Cancellation
- macOS getattrlistbulk Native Scanner
- Carbon App Shell & Layout
- Full Disk Access & IPC Types
- macOS Platform Adapters
- Cleanup Catalog Rules
- Cleanup Confirm Dialog
- ESLint & Workspace Config
- Core Trash Trait Definitions
- Treemap Breadcrumb Component
- Tauri App Configuration
- Permissions IPC Command
- Catalog Candidate Models
- TypeScript Compiler Config
- macOS APFS Size Accounting
- Carbon Theme Provider
- Disk Cleanup Command Handlers
- Cleanup Log Formatting
- macOS Permissions Checker
- Package Scripts & Manifest
- Full Disk Access Banner
- Cleanup Safety Exclusion Rules
- macOS Trash Integration
- macOS File Attributes Extractor
- Disk Scan State Store
- Canvas Treemap Geometry
- Carbon Icon Dependencies
- Vite Node Config
- Filesystem Safety Assertions
- Node Path & Process Helpers
- Tauri Permissions Capabilities
- Memory Sampling Tooling
- Project Documentation & Readme
- Tauri Brand Assets
- Cache Catalog Definition
- macOS App Icon Set

## God Nodes (most connected - your core abstractions)
1. `Tree` - 55 edges
2. `NodeId` - 44 edges
3. `plan_cleanup()` - 27 edges
4. `Guardrails` - 25 edges
5. `CancelToken` - 25 edges
6. `build_view()` - 25 edges
7. `plan_candidates()` - 23 edges
8. `fixture()` - 22 edges
9. `formatBytes()` - 22 edges
10. `make_plan()` - 21 edges

## Surprising Connections (you probably didn't know these)
- `make_guardrails()` --calls--> `app_bundle_or_exe()`  [INFERRED]
  src-tauri/src/commands/cleanup.rs → src-tauri/src/platform/macos/guardrails.rs
- `make_plan()` --calls--> `plan_cleanup()`  [INFERRED]
  src-tauri/src/commands/cleanup.rs → src-tauri/src/core/cleanup.rs
- `run_plan()` --calls--> `apply_results()`  [INFERRED]
  src-tauri/src/commands/cleanup.rs → src-tauri/src/core/cleanup.rs
- `run_plan()` --calls--> `execute()`  [INFERRED]
  src-tauri/src/commands/cleanup.rs → src-tauri/src/core/cleanup.rs
- `run_plan()` --calls--> `count()`  [INFERRED]
  src-tauri/src/commands/cleanup.rs → src-tauri/src/core/view.rs

## Import Cycles
- None detected.

## Hyperedges (group relationships)
- **Safe Cleanup Pipeline & Guardrails** — readme_safety_model, readme_cache_cleanup_catalog, disk_treemap_plan_move_to_trash [INFERRED 0.85]

## Communities (51 total, 12 thin omitted)

### Community 0 - "Core Cleanup & Guardrails"
Cohesion: 0.06
Nodes (77): open(), a_changed_item_type_is_not_moved(), a_path_that_resolves_elsewhere_is_not_moved(), a_selected_folder_absorbs_selected_items_inside_it(), a_sibling_sharing_a_name_prefix_is_not_blocked(), an_interior_dot_is_judged_as_the_path_it_names(), an_item_removed_after_the_preview_is_reported_as_already_gone(), an_unknown_node_is_an_error() (+69 more)

### Community 1 - "Treemap View & Breadcrumbs"
Cohesion: 0.06
Nodes (71): bench_view(), defaults_to_the_root(), find_node_by_path(), get_view(), lookup_path(), MIB, missing_limits_use_defaults(), no_scan_error_when_state_is_empty() (+63 more)

### Community 2 - "Folder Table & Formatting UI"
Cohesion: 0.05
Nodes (66): @carbon/themes, vitest, FolderTable(), FolderTableProps, KIND_LABEL, formatPercent(), UNITS, Crumb (+58 more)

### Community 3 - "Scan Engine & Thread Channels"
Cohesion: 0.07
Nodes (53): ScanOptions, ScanOutput, a_root_chosen_inside_a_cloud_mount_is_scanned_as_asked(), bench_real_dir(), build_fixture(), build_home(), bulk_and_portable_modes_agree(), bulk_mode_records_unreadable_directory() (+45 more)

### Community 4 - "Cleanup Execution Logging"
Cohesion: 0.08
Nodes (45): ItemResult, an_unwritable_location_is_an_error_not_a_panic(), at(), later_runs_append_and_never_overwrite(), lines(), log_moved(), LogLine, nothing_moved_means_no_file_is_created() (+37 more)

### Community 5 - "Arena Tree Storage & Rollup"
Cohesion: 0.10
Nodes (34): adding_child_to_file_panics(), aggregating_into_file_panics(), Children, children_lists_only_direct_children(), deep_chain_rolls_up_without_recursion(), detach_in_a_deep_chain_updates_every_ancestor(), detach_matches_a_fresh_rollup(), detach_refuses_the_root_and_repeats() (+26 more)

### Community 6 - "Cleanup IPC & Plan Commands"
Cohesion: 0.08
Nodes (44): a_failed_item_is_reported_and_stays_in_the_tree(), a_newer_plan_replaces_the_older_one(), a_plan_can_only_run_once(), a_plan_then_execute_moves_items_and_updates_the_tree(), a_rescan_invalidates_the_plan(), an_unknown_plan_id_is_rejected_and_keeps_the_real_plan(), CleanupError, CleanupErrorKind (+36 more)

### Community 7 - "Cleanup Scan State & Errors"
Cohesion: 0.11
Nodes (42): A, a_failed_item_is_reported_and_stays_in_the_list(), a_file_changed_after_the_preview_is_left_alone(), a_new_discovery_invalidates_the_plan(), a_newer_plan_replaces_the_older_one(), a_plan_can_only_run_once(), a_second_search_is_rejected_until_the_first_ends(), a_wrong_plan_id_is_rejected_and_keeps_the_real_plan() (+34 more)

### Community 8 - "Scan IPC & Cancellation"
Cohesion: 0.07
Nodes (30): ActiveGuard, cancel_scan(), io_errors_map_to_structured_kinds(), issues_serialize_with_camel_case_kinds(), make_scanner(), PROGRESS_EVENT, ProgressPayload, register() (+22 more)

### Community 9 - "macOS getattrlistbulk Native Scanner"
Cohesion: 0.07
Nodes (19): ATTR_CMN_ERROR, BUFFER_SIZE, BulkEntry, BulkEntryError, BulkKind, Dir, Other, BulkListing (+11 more)

### Community 10 - "Carbon App Shell & Layout"
Cohesion: 0.14
Nodes (22): react, App(), AppShell(), AppHeader(), AppHeaderProps, AppSideNav(), AppSideNavProps, Explorer (+14 more)

### Community 11 - "Full Disk Access & IPC Types"
Cohesion: 0.11
Nodes (25): FullDiskAccessNotice(), cancelScan(), CandidateCategory, checkFullDiskAccess(), CLEANUP_ERROR_KINDS, CleanupErrorKind, CleanupOutcome, FullDiskAccessStatus (+17 more)

### Community 12 - "macOS Platform Adapters"
Cohesion: 0.12
Nodes (8): BLOCK_UNIT, bytes(), distinct_files_are_each_counted(), hard_links_are_counted_once(), MacSizeAccounting, temp_dir(), uses_allocated_size(), write_file()

### Community 13 - "Cleanup Catalog Rules"
Cohesion: 0.12
Nodes (14): apple_prefixed_names_are_excluded_whatever_the_case(), at(), children_roots_accept_descendants_but_not_the_root_or_outsiders(), NameRules, Offer, Children, Whole, other_names_are_kept() (+6 more)

### Community 14 - "Cleanup Confirm Dialog"
Cohesion: 0.18
Nodes (21): CleanupConfirmModal(), CleanupConfirmModalProps, DoneBody(), errorTitle(), ExcludedList(), pluralItems(), PreviewBody(), PreviewModal() (+13 more)

### Community 15 - "ESLint & Workspace Config"
Cohesion: 0.11
Nodes (19): name, private, type, version, eslint, @eslint/js, eslint-plugin-react, eslint-plugin-react-hooks (+11 more)

### Community 16 - "Core Trash Trait Definitions"
Cohesion: 0.26
Nodes (19): a_missing_root_is_ignored_without_a_trace(), a_symlink_child_is_skipped_and_its_target_is_never_offered(), a_symlinked_root_is_skipped_whole(), a_whole_root_is_offered_once_as_itself(), an_unreadable_folder_is_skipped_and_the_rest_continue(), cancelling_before_or_during_discovery_stops_it(), candidates_are_ordered_largest_first_with_unique_ids_from_one(), candidates_serialize_in_camel_case() (+11 more)

### Community 17 - "Treemap Breadcrumb Component"
Cohesion: 0.19
Nodes (18): cancelDiscovery(), CleanupError, CleanupPlan, CleanupResult, discoverCleanup(), DiscoveryProgress, DiscoveryResult, executeCleanup() (+10 more)

### Community 18 - "Tauri App Configuration"
Cohesion: 0.10
Nodes (19): app, security, windows, build, beforeBuildCommand, beforeDevCommand, devUrl, frontendDist (+11 more)

### Community 19 - "Permissions IPC Command"
Cohesion: 0.15
Nodes (13): check_full_disk_access(), FULL_DISK_ACCESS_SETTINGS_URL, FullDiskAccessStatus, Granted, Missing, Unknown, make_checker(), statuses_serialize_in_camel_case() (+5 more)

### Community 20 - "Catalog Candidate Models"
Cohesion: 0.18
Nodes (16): CatalogRoot, Category, AppCaches, AppLogs, NpmCache, XcodeBuildCache, an_empty_identifier_still_excludes_apple_folders(), apple_folders_and_the_apps_own_folder_are_excluded() (+8 more)

### Community 21 - "TypeScript Compiler Config"
Cohesion: 0.11
Nodes (18): compilerOptions, allowImportingTsExtensions, isolatedModules, jsx, lib, module, moduleResolution, noEmit (+10 more)

### Community 22 - "macOS APFS Size Accounting"
Cohesion: 0.15
Nodes (11): app_bundle_or_exe(), AppPaths, guardrails(), HOME_DIRS, HOME_SECRET_TREES, kind(), protected_paths(), rules_still_cover_the_system_when_home_is_unknown() (+3 more)

### Community 23 - "Carbon Theme Provider"
Cohesion: 0.12
Nodes (17): devDependencies, @carbon/themes, eslint, @eslint/js, eslint-plugin-react, eslint-plugin-react-hooks, prettier, sass (+9 more)

### Community 24 - "Disk Cleanup Command Handlers"
Cohesion: 0.24
Nodes (13): run_discovery(), Cancelled, Candidate, Collector, discover(), Discovery, DiscoveryContext, DiscoveryProgress (+5 more)

### Community 25 - "Cleanup Log Formatting"
Cohesion: 0.23
Nodes (14): CleanupModal(), CleanupModalProps, DoneBody(), errorTitle(), ExcludedList(), pluralItems(), PreviewBody(), PreviewModal() (+6 more)

### Community 26 - "macOS Permissions Checker"
Cohesion: 0.18
Nodes (8): existing_directory_is_accessible(), FULL_DISK_ACCESS_PROBE, MacPermissionChecker, missing_path_is_not_found(), Access, Accessible, Denied, NotFound

### Community 27 - "Package Scripts & Manifest"
Cohesion: 0.14
Nodes (14): scripts, build, check, dev, format, format:check, lint, preview (+6 more)

### Community 28 - "Full Disk Access Banner"
Cohesion: 0.22
Nodes (10): @carbon/icons-react, react-dom, CleanupActionBar(), CleanupActionBarProps, pluralItems(), pluralItems(), SelectionActionBar(), SelectionActionBarProps (+2 more)

### Community 29 - "Cleanup Safety Exclusion Rules"
Cohesion: 0.18
Nodes (8): is_recent(), examine(), Examined, Candidate, Skipped, FakeScanner, FakeSizing, file_name()

### Community 30 - "macOS Trash Integration"
Cohesion: 0.26
Nodes (3): a_missing_path_is_reported_as_not_found(), MacTrashProvider, moves_a_real_file_to_the_trash()

### Community 31 - "macOS File Attributes Extractor"
Cohesion: 0.29
Nodes (6): a_root_inside_a_cloud_location_is_recognised(), CloudFolders, folders(), folders_inside_a_mount_are_not_matched_separately(), ordinary_folders_and_the_parents_are_not(), provider_mounts_and_icloud_drive_are_cloud_folders()

### Community 32 - "Disk Scan State Store"
Cohesion: 0.29
Nodes (9): @carbon/react, ScanIssueList(), ScanIssueListProps, errorTitle(), ScanPanel(), ScanPanelProps, ScanError, ScanIssue (+1 more)

### Community 33 - "Canvas Treemap Geometry"
Cohesion: 0.22
Nodes (8): Disk Treemap Architecture, Arena-based Disk Tree Model, Carbon Canvas Treemap View, Move to Trash Dry-Run Workflow, Platform Abstraction Traits, Webview Root Entrypoint, Carbon & Plex Build Packages Config, Carbon Disk Treemap App

### Community 34 - "Carbon Icon Dependencies"
Cohesion: 0.25
Nodes (8): dependencies, @carbon/icons-react, @carbon/react, react, react-dom, @tauri-apps/api, @tauri-apps/plugin-dialog, @tauri-apps/plugin-opener

### Community 35 - "Vite Node Config"
Cohesion: 0.25
Nodes (7): compilerOptions, allowSyntheticDefaultImports, composite, module, moduleResolution, skipLibCheck, include

### Community 36 - "Filesystem Safety Assertions"
Cohesion: 0.29
Nodes (7): reason_of(), Skipped, SkipReason, OutsideRoot, Recent, Symlink, Unreadable

### Community 37 - "Node Path & Process Helpers"
Cohesion: 0.33
Nodes (3): vite, @vitejs/plugin-react, plexPath

### Community 38 - "Tauri Permissions Capabilities"
Cohesion: 0.33
Nodes (5): description, identifier, permissions, $schema, windows

## Knowledge Gaps
- **88 isolated node(s):** `@tauri-apps/cli`, `@types/react`, `@types/react-dom`, `eslint`, `prettier` (+83 more)
  These have ≤1 connection - possible missing edges or undocumented components. (Counts symbols only; 331 node(s) total have ≤1 connection when file, concept and rationale nodes are included.)
- **12 thin communities (<3 nodes) omitted from report** — run `graphify query` to explore isolated nodes.

## Suggested Questions
_Questions this graph is uniquely positioned to answer:_

- **Why does `Tree` connect `Arena Tree Storage & Rollup` to `Core Cleanup & Guardrails`, `Treemap View & Breadcrumbs`, `Scan Engine & Thread Channels`, `Cleanup IPC & Plan Commands`, `Scan IPC & Cancellation`, `Core Trash Trait Definitions`?**
  _High betweenness centrality (0.070) - this node is a cross-community bridge._
- **What connects `@tauri-apps/cli`, `@types/react`, `@types/react-dom` to the rest of the system?**
  _88 weakly-connected nodes found - possible documentation gaps or missing edges._
- **Should `Core Cleanup & Guardrails` be split into smaller, more focused modules?**
  _Cohesion score 0.05721003134796238 - nodes in this community are weakly interconnected._
- **Why does `NodeId` connect `Arena Tree Storage & Rollup` to `Core Cleanup & Guardrails`, `Treemap View & Breadcrumbs`, `Scan Engine & Thread Channels`, `Cleanup IPC & Plan Commands`, `Scan IPC & Cancellation`, `Core Trash Trait Definitions`?**
  _High betweenness centrality (0.046) - this node is a cross-community bridge._
- **Should `Treemap View & Breadcrumbs` be split into smaller, more focused modules?**
  _Cohesion score 0.05708548479632817 - nodes in this community are weakly interconnected._
- **Should `Folder Table & Formatting UI` be split into smaller, more focused modules?**
  _Cohesion score 0.05450372920252438 - nodes in this community are weakly interconnected._
- **Should `Scan Engine & Thread Channels` be split into smaller, more focused modules?**
  _Cohesion score 0.07108081791626096 - nodes in this community are weakly interconnected._