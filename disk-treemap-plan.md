# Carbon Disk Treemap: Implementation Plan

## Top-Level Overview

**Goal:** A lightweight macOS app that scans disk usage, shows it as an interactive treemap built with the Carbon Design System, and lets the user clean up space safely with Move to Trash and a dry-run preview.

**Confirmed decisions**
- **Stack:** Tauri v2. Rust core with a React + `@carbon/react` UI. Tauri uses the OS webview rather than bundling Chromium, which keeps the app small.
- **Platform:** macOS first. Platform-specific behavior sits behind Rust traits so a Windows adapter can be added later without refactoring shared code.
- **MVP scope (only this):** disk scan, interactive treemap, Move to Trash with dry-run preview.
- **Out of MVP (later):** duplicate detection, stale or unused file analysis, Windows adapters.
- **Large-tree strategy:** the full tree stays in Rust. The UI receives only a small, depth-limited, top-K view. Small files are aggregated per directory under a size threshold, and a later dedupe feature will re-walk the files.

**Non-goals for MVP:** hard delete, selecting individual tiny files, cloud storage, scheduled scans, Windows build.

**Open items to verify during implementation (not assumed):**
- Whether `@carbon/charts` treemap is usable at this scale. The default plan is a custom canvas treemap that uses Carbon tokens.
- Which macOS APIs give the best scan throughput (for example `getattrlistbulk` versus a plain parallel walk). Benchmark before committing.
- Exact size threshold for small-file aggregation and the top-K value. Make both configurable constants and tune them with real disks.

## Architecture (summary)

- **UI layer (shared):** React, Carbon components for the shell, toolbar, breadcrumbs, side panel, modals and notifications. A canvas treemap renders the data.
- **IPC layer:** typed Tauri commands and events. The UI never touches the filesystem.
- **Rust core (shared):** arena-based tree, scan orchestration, view-query logic, cleanup planning.
- **Platform traits (per OS):** scanner, size accounting, trash, permissions. macOS adapter implemented now; Windows later.

Suggested layout:
- `src-tauri/` Rust crate: `core/` (OS-agnostic), `platform/` (trait definitions plus `macos/`), `commands/` (Tauri IPC)
- `src/` React UI: `components/`, `treemap/`, `state/`, `ipc/`, `styles/`

---

## Sub-Tasks

### 1. Project scaffolding and Carbon shell
**Intent:** Establish a working Tauri v2 + React + Carbon baseline that every later task builds on.

**Expected Outcomes**
- App launches on macOS and shows a Carbon-themed empty shell (header, side area, main content).
- Dev and production builds both succeed.
- Carbon theme tokens and IBM Plex fonts load, with light and dark theme support.

**Todo List**
1. Scaffold Tauri v2 with a Vite + React + TypeScript frontend.
2. Install `@carbon/react` and configure SCSS with the Carbon theme setup.
3. Load IBM Plex via the Carbon-supported method.
4. Build the app shell using Carbon `Header` and layout components.
5. Add theme toggle using Carbon theming.
6. Configure Tauri capabilities and permissions to the minimum needed.
7. Set up lint, format and typecheck scripts for both the Rust and TypeScript sides.

**Relevant Context**
- Workspace is currently empty (only `.git/`), so this is a greenfield start.
- Use the Carbon skills (`carbon-builder`, `carbon-styling-practices`, `carbon-accessibility-practices`) when implementing UI.

**Status:** [ ] pending

---

### 2. Rust core data model and platform traits
**Intent:** Define the compact tree and the portability boundary before writing any scanning code.

**Expected Outcomes**
- Arena-based tree type with bottom-up size rollup, using flat storage and integer ids instead of per-node heap objects.
- Directory nodes and large-file nodes are stored individually. Files under the threshold are aggregated into per-directory counters (count and total bytes).
- Trait definitions for scanner, size accounting, trash and permissions, with no OS-specific code in `core/`.
- Unit tests for tree construction and rollup.

**Todo List**
1. Define the node struct and arena (parent, children range, size, kind, name reference).
2. Define the small-file aggregate fields on directory nodes.
3. Implement rollup of sizes and counts.
4. Define platform traits: `Scanner`, `SizeAccounting`, `TrashProvider`, `PermissionChecker`.
5. Add compile-time structure so `platform/macos/` implements the traits and `core/` depends only on the traits.
6. Make threshold and top-K configurable constants.
7. Write unit tests with in-memory fake trees.

**Relevant Context**
- Size accounting must be a trait because it differs by OS (APFS clones, sparse files and hard links on macOS; hard links, junctions and compression on Windows).
- Keep name storage compact (interned or segment-based) to limit memory.

**Status:** [x] complete. Code is in `src-tauri/src/core/` (`tree.rs`, `config.rs`, `scan.rs`) and `src-tauri/src/platform/` (traits in `mod.rs`; `macos/` has `MacSizeAccounting` and `MacPermissionChecker`). `Scanner` and `TrashProvider` have no macOS impl yet (sub-tasks 3 and 7). Defaults: threshold 1 MiB, top-K 64 (tune in sub-task 8).

---

### 3. macOS scanner
**Intent:** Produce an accurate, fast, streaming scan of a chosen folder or volume.

**Expected Outcomes**
- Parallel directory traversal populating the arena.
- Correct handling of symlinks (not followed), hard links (counted once), permission errors (recorded, not fatal), and cross-volume boundaries (configurable).
- Progress events streamed to the UI (items scanned, bytes, current path, partial top-level sizes).
- Cancellable scan.
- Documented benchmark results versus candidate approaches.

**Todo List**
1. Implement the macOS `Scanner` using a parallel walker. Benchmark against `getattrlistbulk` and pick based on measured results.
2. Implement macOS `SizeAccounting` (allocated size versus logical size, hard-link de-duplication by inode).
3. Apply the small-file aggregation rule during traversal.
4. Emit throttled progress events over Tauri.
5. Add cancellation and error collection.
6. Integration tests against a generated fixture directory tree.

**Relevant Context**
- Choice of walker crate and API is an open item. Verify against current crate documentation when implementing.
- Scan results should be visible incrementally so the treemap can render before the scan completes.

**Status:** [-] mostly complete; one item open (see below).

Done: `MacScanner` in `src-tauri/src/platform/macos/scanner.rs`; `start_scan` / `cancel_scan` commands plus `scan-progress` events in `src-tauri/src/commands/scan.rs`; 10 scanner tests on generated fixture trees (aggregation, symlinks not followed, hard links once, permission errors recorded, unreadable root, cancel, progress).

Benchmark (release build, `~/Documents`, ~518k entries, 14 cores, warm cache, median of 3 rounds with modes alternated). Both modes return identical node counts, file counts and byte totals.

| Threads | Portable (`readdir` + `lstat`) | Bulk (`getattrlistbulk`) | Speedup |
|---|---|---|---|
| 1 | 5.67 s | 2.80 s | 2.0x |
| 4 | 2.15 s | 1.07 s | 2.0x |
| 8 | 2.03 s | 1.09 s | 1.9x |
| 14 | 2.16 s | 1.31 s | 1.6x |

`du -sk` took 4.6 s on the same tree. The first single-thread portable run was 9.4 s (cold cache); later runs 5.6 s.

**Decision:** `getattrlistbulk` is the default (`ReadMode::Bulk`). It falls back to the portable reader per directory if the filesystem returns `ENOTSUP`/`EINVAL`. Best throughput is at 4 to 8 threads; 14 is slightly slower, so the default thread count is capped at 8 (`MAX_DEFAULT_THREADS`).

Caveats: numbers are from one machine, one APFS volume and a warm cache. Cold-cache and external or network volumes are untested, and the fallback path is not exercised by any test.

Open:
- Progress events carry items, bytes and current path, but not partial top-level sizes. The tree is only handed back when the scan finishes, so the treemap cannot render incrementally yet.

---

### 4. Permissions and onboarding (macOS)
**Intent:** macOS restricts access to many folders, so the app must explain and handle this gracefully.

**Expected Outcomes**
- App detects missing Full Disk Access and shows a Carbon-based onboarding or guidance panel with a link to System Settings.
- Inaccessible paths appear in the results as "not accessible" instead of causing failures.
- Folder picker for choosing the scan root.

**Todo List**
1. Implement the macOS `PermissionChecker`.
2. Build the onboarding modal or inline notification using Carbon components.
3. Add the root-folder picker and recent-roots list.
4. Surface scan errors in a Carbon-based list.
5. Verify the entitlement and signing requirements for distribution.

**Relevant Context**
- Distribution constraints (sandboxing, notarization) affect what the app can read. Confirm which distribution route is intended before finalizing entitlements.

**Status:** [-] items 1 to 4 complete and confirmed working in the built app; item 5 is partly done and waiting on a decision.

Done:
- `check_full_disk_access` and `open_full_disk_access_settings` commands (`src-tauri/src/commands/permissions.rs`). The notice re-checks when the window regains focus.
- Scan errors are structured (`kind` plus `message`) so the UI never parses text. Cancellation shows as info, not as an error.
- Unreadable paths are listed in an accordion under the scan result (`ScanIssueList`).
- Folder picker uses `tauri-plugin-dialog` with only the `dialog:allow-open` permission. Recent roots (5 max) are kept in `localStorage`.
- Bundle is signed ad hoc with a stable identifier (`com.carbon.treemap`, `bundle.macOS.signingIdentity = "-"`) so macOS lists it under Full Disk Access. Needed because `tauri dev` runs a bare binary that cannot be listed.

Item 5, what is known and what is not:
- Tauri enables hardened runtime by default (`hardenedRuntime: true`). Confirmed in the Tauri config reference.
- Ad-hoc signing changes with each rebuild, so macOS may drop the Full Disk Access grant after a rebuild. A Developer ID certificate gives a stable identity.
- Not verified: the App Store route. My understanding, not checked against Apple documentation, is that it requires the App Sandbox, which would block scanning arbitrary folders. Treat Developer ID plus notarization as the working assumption.
- Not done: no Developer ID certificate or notarization credentials have been configured, so signed and notarized builds are untested. Notarization needs `APPLE_ID`, `APPLE_PASSWORD` and `APPLE_TEAM_ID` (or an API key) at build time.
- No `entitlements` file exists. Whether the app needs any beyond the hardened-runtime defaults has not been tested.

Decision (user): personal use now, with Developer ID outside the App Store as a possible later step. The App Store is not a target. Signing and notarization are therefore deferred; see sub-task 8.

---

### 5. View-query layer (the large-tree solution)
**Intent:** Ensure the UI only ever receives a small slice of the tree, regardless of disk size.

**Expected Outcomes**
- A `get_view` command that takes a node id, depth limit and top-K, and returns a bounded subtree.
- Each directory returns its K largest children plus one aggregated "other" node (item count and size). Aggregated small files are folded into that same node.
- Payload size stays bounded and is documented.
- Path-to-node lookup for breadcrumbs and for restoring state after rescan.

**Todo List**
1. Implement the view-building function over the arena.
2. Expose it as a typed Tauri command.
3. Define serialization with compact fields.
4. Add tests for top-K, the "other" bucket, and size totals matching the unaggregated tree.
5. Measure payload size and latency on a large fixture.

**Relevant Context**
- Sizes in the view must sum to the real parent size, so the treemap stays truthful.

**Status:** [x] complete on the Rust and IPC side. No UI consumes `get_view` yet; that is sub-task 6. The "visible incrementally during a scan" item carried over from sub-task 3 is NOT done (see below).

Done:
- `core/view.rs`: `build_view`, `breadcrumbs`, `find_by_path`. Commands `get_view` and `find_node_by_path` in `commands/view.rs`; typed wrappers in `src/ipc/index.ts`.
- Each expanded directory lists its top-K children (largest first, ties by name) plus one `other` node that holds the omitted children and the directory's aggregated small files. For every expanded directory the children's sizes and file counts sum exactly to the directory's. This is tested on fixed and pseudo-random trees across several depth, top-K and budget settings.
- A node budget (`MAX_VIEW_NODES = 5000`) bounds the payload. Depth and top-K alone allow (K+1)^depth nodes, so they cannot bound it. Expansion is breadth first, and a directory is expanded only if all its children fit in the remaining budget. Otherwise it keeps its true size and is flagged `expandable` for a separate request.
- Limits received over IPC are clamped (`MAX_VIEW_DEPTH = 6`, `MAX_TOP_K = 256`). Unknown node ids and "no scan yet" return typed errors.
- Raw node ids are only valid for the scan that produced them. After a rescan the UI must use `find_node_by_path` to restore position.

Measured (release build, `~/Documents`, 41,728 tree nodes):

| Request | Nodes | JSON | Build | Encode |
|---|---|---|---|---|
| default (depth 2, K 64) | 250 | 25 KB | 43 us | 24 us |
| depth 1, K 64 | 40 | 3.8 KB | 3 us | 3 us |
| depth 3, K 64 | 796 | 73 KB | 59 us | 54 us |
| depth 6, K 256 (maximum) | 5,000 | 438 KB | 460 us | 308 us |

The worst case a caller can request is about 440 KB and under 1 ms on this tree. Payload is bounded by the budget, not by disk size, but this was measured on one 41k-node tree only; the plan's goal of a much larger fixture is not covered. Build time grows with the size of each directory scanned for top-K, not with the whole tree.

Open:
- Sizes below the 5,000-node budget are exact, but a directory the budget could not expand shows as one block. The UI should request it on drill-down.
- Incremental view during a scan: the tree is still only available after the scan finishes. Doing this needs the tree shared behind a lock while the scanner writes to it, which was not attempted.
- Files folded into an aggregate have no node, so they cannot be found by path, and later cleanup (sub-task 7) cannot select them individually. This matches the stated non-goal.


---

### 6. Interactive treemap component
**Intent:** Render and navigate the treemap smoothly with a custom canvas component styled with Carbon tokens.

**Expected Outcomes**
- Squarified treemap layout rendered to canvas.
- Rectangles smaller than a few pixels are merged visually, so draw cost depends on screen size and not disk size.
- Hover tooltip (path, size, percentage), click to drill down, breadcrumb navigation back up, and selection.
- Color mapping by file kind or depth using Carbon tokens, working in light and dark themes.
- Keyboard-accessible alternative: the same data available as a Carbon `DataTable` or tree list beside the canvas.
- Handles resize and high-DPI displays.

**Todo List**
1. Implement the layout (evaluate a d3-hierarchy squarify layout versus a custom one).
2. Build the canvas renderer with hit testing on a layout index.
3. Wire drill-down and breadcrumbs to the `get_view` command.
4. Add tooltips, selection state and selection summary (count and total size).
5. Add the accessible table or list view with keyboard navigation and screen reader labels.
6. Verify the contrast of treemap colors against WCAG 2.2 AA.
7. Check whether `@carbon/charts` treemap is viable at this scale. Adopt it only if it clearly meets the performance bar.

**Relevant Context**
- Canvas content is not accessible by default, which is why the table or list alternative is part of this task.

**Status:** [x] complete in code; **not yet checked by eye in the running app** (see "Not verified"). The checks that ran: typecheck, lint, format, 64 unit tests, production `vite build`.

Done:
- Layout: own squarified implementation in `src/treemap/layout.ts` (no d3 dependency; it is about 100 lines and is tested for exact area conservation, no overlap, and bounds). Pure functions, so it is unit-tested without a DOM.
- Tiny tiles: within each directory, any child whose tile would be under `minArea` (36 px²) is folded, together with the backend's `other` node, into one "Smaller items" tile. Sizes and file counts are conserved. Draw cost is therefore capped by screen area: a test lays out 5,000 nodes and asserts the tile count stays under `width x height / minArea`.
- Canvas: `src/treemap/TreemapCanvas.tsx` and `render.ts`. High-DPI via `devicePixelRatio` and a `ResizeObserver`. Hit testing walks the flat tile list from the end, so the deepest tile wins. Hover tooltip shows path, size, share of the current folder and file count.
- Navigation: `src/state/useExplorer.ts` calls `get_view`. Double-click a folder, a table button, or a breadcrumb opens it. A stale-response guard drops results from superseded requests. After a rescan, the previous location is restored through `find_node_by_path`; the selection is cleared because raw ids are not valid across scans.
- Selection: click selects, shift or command click toggles. Summary shows item count and bytes, counting an item once when an ancestor is also selected. Only real files and folders can be selected, not "Smaller items" or inaccessible entries.
- Accessible alternative: `FolderTable` (Carbon `Table` with select-all, per-row checkboxes, and an "Open folder" button per row). It lists the same children that the canvas draws, largest first, with a screen-reader summary. The canvas is `role="img"` with a label pointing to the table.
- Colors: read at draw time from Carbon CSS custom properties (`--cds-tag-background-*`, `--cds-tag-color-*`, `--cds-focus`), so the canvas follows the theme with no palette of its own.
- Contrast: tests compute WCAG ratios from `@carbon/themes` for the `white` and `g100` themes. Label text on tile fill is at least 4.5:1, and the selection ring and hover outline are at least 3:1 against every tile fill, in both themes.
- Layout cost at the largest payload (5,001 nodes, synthetic tree, Node, mean of 20 runs): 0.9 ms at 1200x700 and 0.5 ms at 2400x1400. Canvas paint time was not measured.

Bug found and fixed while doing this: the theme toggle set `data-carbon-theme` on `<html>`, but the compiled Carbon CSS defines its tokens under `.cds--white` and `.cds--g100` classes only (checked in the built CSS). The attribute selector does not exist, so dark mode would not have re-themed Carbon tokens. `ThemeContext` now also sets the `cds--<theme>` class on `<html>`. Without this the canvas, which reads those tokens, could not follow the theme.

`@carbon/charts` treemap (todo 7): not adopted. Its published options type, `TreemapChartOptions`, is an alias of `BaseChartOptions` with no treemap-specific settings. There is no drill-down, selection, depth or minimum-tile control, and the only accessibility option is an SVG `aria-label`. That does not cover the requirements above. I did not benchmark it, so I make no claim about its speed.

Not verified:
- Nothing has been looked at in the running Tauri app. Visual quality, label legibility, tooltip placement and the dark theme toggle on real data are untested by eye.
- Pointer behavior (hover, click, double-click) and the ResizeObserver path have no automated test; only layout, selection, palette and contrast do. There is no DOM test environment.
- Contrast is computed for tile fill versus label text and the outlines. The seam between adjacent tiles is not checked.
- The table is a plain, non-virtualized list capped by the view's top-K (at most 257 rows). Sorting is fixed to size, largest first.
- A folder the budget could not expand is drawn as one block and opens on double-click. Its tooltip says "Double-click to open".
- Keyboard users reach everything through the table. The canvas itself is not focusable.

---

### 7. Cleanup: dry-run preview and Move to Trash
**Intent:** Let the user act on what they see, safely and reversibly.

**Expected Outcomes**
- User selects one or more items in the treemap or list and opens a preview of exactly what will be moved to Trash (paths, sizes, total reclaimable space).
- Confirm step in a Carbon modal. No hard delete anywhere in the app.
- Items are moved using the macOS Trash API through the `TrashProvider` trait.
- Per-item success or failure results are reported, and the tree and sizes update afterward.
- Guardrails: block system-critical paths and the app's own data, and warn on paths outside the user's home.

**Todo List**
1. Implement the `TrashProvider` for macOS.
2. Implement a cleanup planner in `core/` that builds the dry-run result and applies the guardrail list.
3. Expose plan and execute as separate Tauri commands, where execute only accepts a plan the user confirmed.
4. Build the preview and confirmation modal with Carbon components.
5. Update the in-memory tree after the move without requiring a full rescan, and offer a manual rescan.
6. Add Carbon toast or inline notifications for results and failures.
7. Tests for guardrails, partial failure and size recalculation.

**Relevant Context**
- Reversibility (Trash) is the main safety property. Do not add a permanent delete option in the MVP.

**Status:** [x] complete in code; **the flow has not been exercised in the running app** (see "Not verified"). The checks that ran: `pnpm check` (typecheck, ESLint with zero warnings, 64 vitest tests, rustfmt, clippy `-D warnings`), `pnpm build`, and `cargo test` (122 passed, 3 ignored).

Done:
- Trash: `MacTrashProvider` (`platform/macos/trash.rs`) behind the `TrashProvider` trait, using the `trash` crate 5.2.9 (new macOS-only dependency) with `DeleteMethod::NsFileManager`. This needs no Automation permission prompt. There is no hard-delete code path anywhere.
- Guardrails (`core/cleanup.rs`, `platform/macos/guardrails.rs`): blocked are system trees and directories, the home folder and its standard folders, `.ssh`, `.gnupg`, Keychains, `~/.Trash`, the app's own data (WebKit, Preferences, Caches for `com.carbon.treemap`) and the running app bundle. Paths outside the home folder are allowed only with an explicit acknowledgement. Relative paths and `..` are rejected.
- Planner: `plan_cleanup` merges a selected child into a selected parent, reports included and excluded items with a reason for each, and computes the reclaimable total from scan sizes.
- Commands (`commands/cleanup.rs`): `plan_cleanup_items` and `execute_cleanup` are separate. A plan lives in Rust, is single-use, and is invalidated by a rescan or a newer plan. Execute re-checks guardrails, the resolved path and the item type, and is refused while a scan runs. Limit: 1,000 items per plan. Error kinds: `noScan`, `unknownNode`, `tooMany`, `stalePlan`, `needsAcknowledgement`, `scanRunning`.
- In-place update: `Tree::detach` unlinks the subtree and subtracts its size and file count from every ancestor. Other node ids stay valid. Cost is O(siblings + depth + subtree). Failed items stay in the tree. Items already gone are removed from it.
- UI: "Preview move to Trash" button next to "Clear selection". `CleanupModal` is a Carbon danger modal that opens with focus on Cancel. It lists included and excluded items, shows the total, requires a checkbox for outside-home items, then shows per-item results and a "Rescan folder" button. Moved items leave the selection and the current folder reloads, falling back to the root if it is gone. Results are shown inline in the modal, not as toasts.
- Tests (Rust, with fake providers): guardrail verdicts, parent/child merging, partial failure, size and file-count recalculation after detach, stale and single-use plans, acknowledgement refusal putting the plan back.
- One `#[ignore]`d test calls the real Trash. I ran it once and it passed. It left one tiny temp file in the Trash.

Not verified:
- Nobody has opened the modal in the running app. Focus on Cancel, the acknowledgement checkbox, layout of long paths and the dark theme are untested by eye. There is no DOM test environment.
- The end-to-end path (select, plan, execute, tree update, refresh) has not been run. The real Trash was only exercised for one temp file, through `MacTrashProvider` alone.
- "Put Back" is probably unavailable for items moved with the `NSFileManager` method. The items are in the Trash, but restoring them to the original place may need a manual move.
- Reclaimed space only appears after the Trash is emptied. The sizes shown are scan-time sizes, so they can be stale.
- Matching is case-insensitive, which can over-block on a case-sensitive volume.
- The generation check and single-use plan are tested with fakes only, not under real concurrent scans.
- Files folded into small-file aggregates cannot be selected (already a stated non-goal).
- The guardrail list is judgement-based and may be incomplete. iCloud Drive and external volumes were not tried.

---

### 8. Performance, hardening and packaging
**Intent:** Confirm the "extremely lightweight" requirement with measurements and prepare a distributable build.

**Expected Outcomes**
- Measured numbers for bundle size, idle memory, memory during a large scan, and scan time on a large real disk. Record them in this plan.
- No UI jank during scanning and drill-down.
- Signed and notarized macOS build, if distribution is intended.
- Documentation of the platform-trait boundary and the steps to add a Windows adapter.

**Todo List**
1. Build a repeatable benchmark (fixture plus real-disk run).
2. Profile and fix hot spots in scan, view-building and rendering.
3. Tune the threshold and top-K constants with real data.
4. Accessibility pass (keyboard, focus, screen reader, contrast).
5. Configure signing and notarization.
6. Write a short architecture and extension note covering the Windows port path (scanner, size accounting, Recycle Bin provider, permissions, long paths).

**Relevant Context**
- Windows later: its adapters and testing on the WebView2 engine are explicitly deferred, but the trait boundaries from sub-task 2 must not leak macOS-specific types.

**Status:** [-] in progress. Size, idle memory and scan-memory measurements are taken. Tuning, the accessibility pass, signing and notarization, and the Windows note are not done. See "Open" below.

Measured so far (release build, one machine with 14 cores, APFS; `ps` for RSS and `/usr/bin/time -l` for the Rust test binary):

| Measure | Result |
|---|---|
| Release binary | 4.4 MB |
| `.app` bundle (ad-hoc signed, `pnpm tauri build --bundles app`) | 5.7 MB |
| Frontend `dist/` | 3.0 MB (JS 405 KB, 123 KB gzip; CSS 1.2 MB, 114 KB gzip; the rest is IBM Plex font files) |
| Idle, window open, 8 s after launch, no scan (`ps` RSS, first sample) | Rust process 106 MB, WebKit WebContent 80 MB, GPU 29 MB, Networking 15 MB. About 230 MB summed. Overstated, see below. |
| Idle, window open, about 10 to 90 s after launch, no scan (`footprint`, the figure Activity Monitor shows as Memory) | Rust process 34 MB, WebContent 51 to 82 MB, GPU 14 MB, Networking 6 MB. **About 105 MB total** in the last sample. WebContent ranged 51 to 82 MB between two launches, so treat it as a range. |
| Scan of `~/Documents` (42,259 tree nodes, 20 GB) then `get_view` | 27 MB max RSS, 20 MB peak footprint. The run took 1.4 s. |
| Scan of `$HOME` (323,407 tree nodes, 349 GB) | 112 MB max RSS, 66 MB peak footprint. |
| `get_view` at the maximum request (5,000 nodes) | 438 KB JSON, 0.5 ms build, 0.34 ms encode. Unchanged from sub-task 5. |

Caveats on these numbers:
- Resolved: the 106 MB Rust figure was RSS, which counts shared mapped pages. Its `footprint` is 34 MB, of which about 18 MB is small malloc blocks and 17 MB is clean mapped files (shared). I did not trace the 18 MB of malloc to specific allocations, so I cannot say how much of it Tauri, WebKit bindings or our code own. Use footprint, not RSS, for comparisons. `scripts/sample-memory.sh` does this.
- The first GPU-process reading was 216 MB, and 5 s later 14 MB. I did not find out why. Treat single GPU samples with care.
- Memory during a scan comes from the Rust test binary only (max RSS, so an upper bound on what the Rust side holds). It excludes the webview. I could not drive the app UI from here (macOS refused UI scripting), so footprint during a scan and after drill-down in the running app is **not measured**. Run `scripts/sample-memory.sh 2` while scanning and drilling down.
- Nothing was timed on a cold cache, an external volume or a network volume.

Slow `$HOME` scan, investigated: the scan took **526 s wall-clock** with only 1.5 s of user CPU and 39 s of system time, against 1.4 s for `~/Documents`. I timed each top-level folder of `$HOME` separately (90 s cap each). Every folder finished in about 1 s or less except `~/Library` (hit the cap) and `~/.Trash` (fails at once on permissions). Inside `~/Library`, the only slow entry was `~/Library/CloudStorage`, which holds the Box Drive File Provider mount. Inside it, `Bob Prototypes`, `Mas 9.3` and `Maximo Application Suite` did not finish in 12 s while the other folders took under 1 s, and a `node_modules` and a `.git` directory under `Bob Prototypes` each took over 8 s for `find` alone.
- The slowness is not specific to our reader. `find` and `ls` are slow on the same paths, and listing a few not-yet-opened `node_modules` subfolders in Box took about 0.4 to 2 s each.
- Likely cause, not proven: Box Drive fetches directory metadata on first access, which costs latency per directory. A tree with thousands of small directories (`node_modules`) multiplies it. The files there show the `dataless` flag, so they are cloud placeholders. Our scan does not read file contents, so it should not download them. I did not confirm that.
- My serial-versus-parallel listing test was inconclusive (different directory sets, and the serial set may have been cached), so I make no claim about whether more threads help here.
- Consequence: scanning a home folder that includes a synced cloud folder could take minutes, mostly waiting.

Fix, decided by the user: cloud-sync folders are skipped by default, with an opt-in.
- `platform/macos/cloud.rs` recognises them by path only (no filesystem calls): each direct child of `~/Library/CloudStorage` (Box, Dropbox, OneDrive, Google Drive and similar File Provider mounts) and `~/Library/Mobile Documents` (iCloud Drive). A folder is skipped at the moment its parent is listed, so it is never opened.
- `ScanOptions.include_cloud_folders` (default `false`) and the `start_scan` argument `includeCloudFolders` control it. `ScanOutput.skipped_cloud_folders` and `ScanSummary.skippedCloudFolders` report what was left out.
- A root the user picks inside a cloud location is scanned as asked and nothing under it is skipped.
- UI: a Carbon `Toggle` "Cloud-synced folders" (Skipped / Included (can be slow)) in the scan panel, remembered in `localStorage`. After a scan, an info notice names the skipped folders and says they are not in the totals.
- Tests: 5 scanner tests on a fake home with a `CloudStorage` mount (skipped and reported, included when enabled, total differs, chosen root inside a mount is scanned, the portable reader skips too), 4 path tests, and 1 option-default test. `cargo test` reported 132 passed, 3 ignored.
- Result on the same machine and `$HOME`: **9.3 s** instead of 526 s (323,407 nodes before, 313,450 now; total bytes 349.34 GB before, 349.24 GB now; max RSS 109 MB). One run each, warm cache.
- Not verified: the toggle and notice have not been looked at in the running app. Only the Box mount under `CloudStorage` exists on this machine, so the iCloud Drive path rule and other providers are untested against real data. iCloud "Desktop and Documents" sync keeps files in the normal `~/Desktop` and `~/Documents`, so those are still scanned and may be slow on a machine where files are evicted. Cloud folders reached through a symlink or a different mount path are not recognised. The notice lists full paths and could get long with many mounts. The treemap shows `Library/CloudStorage` as smaller than Finder does, because its children are missing.

Page scroll position now survives opening a folder (`useScrollMemory`, requested by the user). It is checked by typecheck and lint only; I have not seen it in the running app. A newly opened folder inherits the current scroll position; it is not remembered per folder.

Distribution & Packaging:
- Developer certificate configured in `src-tauri/tauri.conf.json`: `"Apple Development: teaquangburo@gmail.com (BBCS9PMAN9)"`.
- Built and signed release packages via `pnpm tauri build`:
  - `.app` bundle: `src-tauri/target/release/bundle/macos/Carbon Disk Treemap.app` (5.72 MiB, hardened runtime enabled, signed).
  - `.dmg` installer: `src-tauri/target/release/bundle/dmg/Carbon Disk Treemap_0.1.0_aarch64.dmg` (3.44 MiB, signed).
- Stable signature with Apple Development identity prevents macOS from dropping Full Disk Access permissions across builds.
- Notarization is deferred (requires `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` environment variables for Gatekeeper notarization on third-party machines).

Accessibility pass (code review plus small fixes; **nothing tested with VoiceOver, a keyboard, or an automated checker**):
- Fixed: the "Skip to main content" link pointed at `#main-content`, but no element had that id, so it did nothing. `<Content>` now has `id="main-content"`.
- Fixed: after opening a folder the clicked row or link is replaced, so keyboard focus fell to the page. An off-screen heading ("Showing <folder>, <size> in <n> files") now takes focus only if focus was lost, and gives a screen reader something to announce. A user who still has focus on a breadcrumb keeps it.
- Fixed: the selection summary was a live region that also held the buttons, so a screen reader could re-read the buttons on every change. Only the count text is the live region now.
- Fixed: the recent-folder chips had the label "/Users/..." only; they now say "Scan <path>".
- Already in place: all treemap actions are reachable through the table (checkboxes, "Open folder" buttons, select all); the canvas has a label pointing to it; Carbon modals trap focus; the cleanup modal puts initial focus on Cancel; theme toggle and menu button have labels; contrast of tile text and outlines is unit-tested for the two themes.
- Not done or not known:
  - The canvas is not focusable, by design; the table is the keyboard path. Whether that is acceptable to a keyboard-only or VoiceOver user is untested.
  - The scan progress bar and the "Scan complete" notice rely on Carbon's own roles; I did not check that a screen reader announces them.
  - The `Toggle` I added has an `A/B` label pair rather than a single state label. I did not check how it reads.
  - The header link `HeaderName href="#"` and side-nav links `href="#"` go nowhere ("Recent scans" is a placeholder from the scaffold).
  - Reduced-motion, zoom to 200 percent, and the 24 px minimum target size (WCAG 2.2) were not checked.
  - The dark theme has not been looked at on the treemap or the modal.

Open:
- Look at the new cloud-folders toggle and notice in the running app, and try a scan with the toggle on.
- Measure footprint and UI responsiveness during a scan and after drill-down in the running app, by hand with `scripts/sample-memory.sh 2`.
- Run the accessibility pass for real: keyboard only, VoiceOver, and an automated check such as axe in the running app.
- Tuning of the threshold and top-K constants, and the Windows-port note.
- Signing and notarization only if you decide to distribute (see above).

---

### 9. Safe cache cleanup (header menu)
**Intent:** Find regenerable cache and log folders, show what they are and how much space they take, and move the ones the user approves to the Trash. Safety comes first: the feature only proposes paths from a fixed catalog, never "anything that looks unused".

**Scope (decided by user)**
- **In scope now: the Safe tier only.** Caches and logs that apps rebuild on demand.
- **Deferred, not designed here:** the Review tier (`node_modules`, Rust `target/`, old installers, `iOS DeviceSupport`), report-only items, and **duplicate cleanup**. Duplicate cleanup stays in "Future Phases" because it needs hashing and a "which copy do I keep" decision.
- The design below does not rule these out later, but nothing in this sub-task builds toward them. No `Tier` enum, no tier UI. Add them when they are scheduled.

**Safety model**
1. **Allowlist, not heuristics.** A fixed catalog of known locations. Nothing outside it can become a candidate.
2. **The UI never sends a path.** Rust discovers candidates, keeps them with ids, and the UI sends back only ids. Same model as `plan_cleanup_items` / `execute_cleanup` today.
3. **Trash only, no hard delete.** Same invariant as sub-task 7. The existing `TrashProvider` is reused unchanged.
4. **Discover, preview, confirm, execute.** The plan is single use and is invalidated by a new discovery, a second execute, or a treemap scan.
5. **Re-check at execution time.** Guardrails, resolved path, item type and the "changed since preview" check run again, via the existing `move_one`.

**Safe-tier catalog (every path must be verified on a real machine before it ships)**

Candidates are the **direct children** of these folders, one candidate per child, never the parent folder itself:

| Category | Location | Notes |
|---|---|---|
| App caches | `~/Library/Caches/*` | Includes Homebrew and pip caches, which live here. `com.apple.*` and this app's own folder are excluded. |
| App logs | `~/Library/Logs/*` | `com.apple.*` excluded. |
| Xcode build cache | `~/Library/Developer/Xcode/DerivedData/*` | Only offered if the folder exists. |
| npm cache | `~/.npm/_cacache` | One candidate. Only offered if it exists. |

Not in the first cut, pending a check of what is on the machine and how each tool behaves: pnpm store (shared and hard-linked), cargo registry cache, other package managers.

**Never a candidate (on top of the existing guardrails)**
- `~/Library/Application Support`, `Preferences`, `Keychains`, `Containers`, `Group Containers`, `Mail`, `Photos`, and any document folder. They are not in the catalog, and a test proves it.
- `com.apple.*` folders. Some belong to system services (iCloud sync, Spotlight) and removing them can cause a resync or confusing errors. Judgement call; revisit with evidence.
- This app's own cache and data (already protected).
- Anything reached through a symlink. The catalog root and each candidate are resolved, and a candidate whose resolved path leaves its catalog root is dropped. Symlinks inside a candidate are not followed when sizing.

**Extra checks for cleanup (not in sub-task 7)**
- **Recently modified.** Skip candidates modified within a recent window (constant, default 24 hours), since they are likely in use. The scanner records no modification time (checked: nothing in `scanner.rs` or `scan.rs` stores one), so this needs its own `symlink_metadata` call per candidate. Directory mtime changes only when direct entries change, so it is a weak signal for a cache with nested folders. Treat it as a partial safeguard, not a guarantee.
- **Running apps.** Show a "quit the app first" note for the Safe tier. Detecting a specific running app from Rust is not decided or verified, so v1 uses a static note.
- **Sizing.** Reuse `MacScanner` on each candidate folder (hard links once, symlinks not followed) instead of a second walker.
- **Permissions.** An unreadable candidate is listed as "not accessible" and cannot be selected. Reuse the Full Disk Access notice.

**Expected Outcomes**
- A "Clean up caches" entry in the header menu opens the cleanup view. It works with or without a finished treemap scan.
- Discovery runs in the background with progress, can be cancelled, and never modifies anything.
- Results are grouped by category with size, item count and a one-line "what happens if I remove it". Full paths are available on demand.
- A confirm modal lists the exact items and the total, and says that space is freed only after the Trash is emptied.
- Per-item results (moved, already gone, failed with reason) and a total moved.
- A local log of what was moved and when (original path, size, time), because "Put Back" may be unavailable for items moved with `NSFileManager`.
- Selection starts empty unless you decide otherwise (see "Pre-selection" below).

**Architecture**
- `core/cleanup_catalog.rs` (OS-agnostic): `Category`, `Candidate { id, category, path, size, files, mtime }`, plus the filtering rules. It takes catalog roots from the platform layer and does no OS-specific work.
- `platform/macos/catalog.rs`: the macOS roots above and the `com.apple.*` rule. A Windows catalog would be a separate file later.
- `commands/cleanup_scan.rs`: `discover_cleanup` (async, cancellable, emits progress), `plan_cleanup_candidates(ids)`, then an execute command.
- **Design choice (resolve while implementing):** `PlanItem.node` is a tree node id and `apply_results` detaches tree nodes, but candidates are not tree nodes. Prefer a separate candidate plan type that shares `move_one` / `execute`. That leaves the tested tree path untouched.
- If a treemap scan exists, tell the user to rescan after a cleanup. Do not edit the tree by path.
- UI: `state/useCleanupScan.ts`, `components/CleanupView.tsx`, `CleanupConfirmModal.tsx` (same Carbon danger-modal pattern, initial focus on Cancel). View switching is a state value (`'treemap' | 'cleanup'`), not a router, so a scan in progress survives switching.

**Header menu (UI shell)**
- Carbon `HeaderNavigation` with two first-level `HeaderMenuItem`s: "Treemap" and "Clean up caches" (changed by user from a "Tools" dropdown). Mark the current view with `isActive` (`isCurrentPage` is deprecated in the installed Carbon).
- Carbon hides `HeaderNavigation` at narrow widths, so mirror the entries in the side nav, which already opens from the existing menu button. Do not reuse the `href="#"` placeholders; use `onClick` handlers.
- Assumption: "shell menu on the header" means navigation in the Carbon header bar. A global-action icon with a `HeaderPanel`, or only a side-nav link, would also fit this plan.

**Todo List**
1. Write the catalog model and filtering rules with unit tests (no I/O in the rules).
2. Add the macOS roots. Check each one on a real machine and record which exist and how big they are.
3. Implement discovery: enumerate roots, apply exclusions and the recent-use window, size with `MacScanner`, support cancellation and progress.
4. Implement the candidate plan and execute path, reusing `move_one` and the guardrails. Keep single-use plans and the item cap.
5. Add the cleanup log in the app data directory.
6. Add `HeaderNavigation`, the side-nav mirror and view switching.
7. Build `CleanupView` (category groups, selection, size totals, empty and error states) and the confirm modal.
8. Accessibility: keyboard-only use, focus returns to the menu after the modal closes, announced progress and results, contrast in both themes.
9. Run the tests below.
10. Manual run on the real home folder: dry run first, then one small real move. Record results here.

**Tests required before this is called done**
- Fake home: each catalog category is found; folders outside the catalog are never offered.
- Exclusions: `com.apple.*`, the app's own caches, recently modified items.
- A candidate that is a symlink leaving its catalog root is dropped. A symlink inside a candidate pointing at a protected path is not followed and not moved.
- The UI cannot name a path: an unknown or stale candidate id is rejected, a second execute of one plan is rejected, and a new discovery invalidates the previous plan.
- Partial failure leaves the other items moved and reports each result.
- The parent folders (`~/Library/Caches` itself) are never candidates.
- A source search confirms no permanent-delete call (`remove_file`, `remove_dir_all`) outside tests.
- Guardrails still block system paths and the home standard folders.

**Risks and open questions**
- **Trash does not free space.** The user moves GBs to the Trash and sees no change until emptying it. Recommended for v1: keep the no-hard-delete rule and say so plainly in the modal and result. An "Empty Trash" step would break that rule, so it needs your explicit decision.
- **Pre-selection.** Should the Safe tier start fully selected, or empty? Recommendation: empty in the first version, with a "Select all" control, until it has been used on real data. Needs your call.
- **Put Back / undo.** Restoring may be manual. The log records original paths. Whether the `trash` crate can report where an item landed on macOS is unchecked.
- **Cache contents are app-specific.** Removing an active cache can disturb a running app. The recent-use window is weak (see above) and the running-app note is static, so these reduce the risk but do not remove it.
- **The 24-hour window may hide most results.** Active apps touch their cache folders constantly. Tune it with real data before trusting the default.
- **Catalog drift.** Tool locations change between versions. Every path is checked, not assumed.
- **Scale.** `~/Library/Caches` can hold very many files. Discovery must stay cancellable and not block the UI. Not measured yet.
- **Cloud folders.** The catalog is local-path only and never enters the cloud-sync mounts skipped in sub-task 8.

**Status:** [-] in progress. Todos 1 to 7 done in code and 9 run; **8 and 10 not done**. The feature has never been run in the app.

Decision (user): the shared DerivedData folders (`ModuleCache.noindex`, `SDKExplicitPrecompiledModules` and similar) stay as separate candidates.

Done (todos 6 and 7):
- `src/ipc/index.ts`: wrappers `discoverCleanup`, `cancelDiscovery`, `planCleanupCandidates`, `executeCleanupCandidates`, `onCleanupProgress`, and the matching types. `ReasonKind` gained `'changed'`, and `CleanupErrorKind` and the `toCleanupError` list gained `noDiscovery`, `unknownCandidate`, `discoveryRunning`, `cancelled`. The shapes were written from the Rust structs, not from a live call.
- `AppHeader.tsx`: `HeaderNavigation` with a "Treemap" item and a "Tools" `HeaderMenu` holding "Clean up caches". `AppSideNav.tsx` has the same two entries, because Carbon hides the header navigation at narrow widths. Both use `onClick` with `preventDefault`, not the old `href="#"` placeholders. The current view is marked with `isActive` and `aria-current="page"`. `src/views.ts` holds the `AppView` type.
- `App.tsx` switches views with state, not a router. The scan, the treemap state and the cleanup list live in `AppShell`, so they survive switching. Only the screen is swapped, so the treemap canvas is recreated when you come back. After a switch, focus moves to the new page heading.
- `state/useCleanupScan.ts`: search (cancellable, with progress), selection, plan, confirm. Selection starts empty, and a new search clears it. After a move, moved and already-gone items leave the list and the selection; failed ones stay. It sends only candidate ids.
- `components/CleanupView.tsx`: items grouped by category with a one-line effect, a count and size per group, "Select all" and "Clear selection", a live selection total, an empty state, an error state, the "quit the apps first" note, and notes for recent, unreadable and left-out items. `CleanupConfirmModal.tsx` is a danger modal with initial focus on Cancel, the outside-home acknowledgement, the "space is freed only after you empty the Trash" text, per-item results, a warning when `logSaved` is false, and a "scan again" note. After the modal closes, focus goes to the review button, or to the page heading if that button is disabled.
- `eslint.config.js`: added the `HTMLButtonElement` global, following the existing one-by-one list.

Checks that ran (todo 9): `pnpm check` (typecheck, ESLint with zero warnings, 64 vitest tests, rustfmt, clippy `-D warnings`), `pnpm format:check`, `pnpm build`, and `cargo test` (193 passed, 4 ignored). A source search found no `remove_file`, `remove_dir_all` or `remove_dir` call outside `#[cfg(test)]` modules in `src-tauri/src`; I compared each hit's line number with its file's `#[cfg(test)]` line.

Not verified:
- **Nothing has been seen in the running app.** There is no DOM test environment, so none of the components has a test. Layout, the header navigation items, the side-nav entries, the modal, the progress bar and both themes are unchecked.
- The header items are plain links with an `onClick` that calls `preventDefault`. I did not try them with a keyboard or check how the header looks with both items at different window widths. The header navigation hides at narrow widths and the side nav then carries the entries; that switch point was not looked at.
- Todo 8 is only partly done in code. Focus moves to the page heading on a view switch and back to the review button when the modal closes. The "found" summary (`role="status"`) and the selection total (`aria-live="polite"`) are live regions. The result shown in the modal is a Carbon `InlineNotification`; I did not check whether it is announced when it appears. A screen reader, keyboard-only use and contrast in both themes have not been tried.
- The search is a single call, so progress shows only a "found so far" count and the path is not shown.
- Selecting a large number of rows renders one checkbox per row. With 131 candidates on this machine that is likely fine, but a very long list was not tried. The Rust cap is the existing per-plan item limit; hitting it shows a "too many items" error.
- A search that returns while the modal is open is not possible (the buttons are disabled), but I did not test that path.
- The "Last changed" column uses the browser's locale date. The "Size is a minimum" tag is shown only when discovery flagged the folder as incomplete.
- Todo 10 (dry run on the real home folder, then one small real move to the Trash) is not done. The real Trash has not been used by this flow.

Done (todo 5):
- `core/cleanup_log.rs`: `log_moved` appends one JSON line per item that was moved (time, original path, name, size, file count) to `cleanup-log.jsonl` in the app data directory. Failed and already-gone items are not logged. It only creates the file and appends to it; it never reads, rewrites or deletes anything else.
- `execute_cleanup_candidates` writes the log after the move. If writing fails, the move still stands and the response sets `logSaved: false`, so the UI can tell the user the record is missing. Logging cannot turn a successful cleanup into an error.
- 5 tests (only moved items, parent folder created, appends not overwrites, no file when nothing moved, unwritable location returns an error). Full suite 193 passed, 4 ignored; clippy and fmt clean.
- Not done: nothing reads the log back, and there is no "view history" screen. The path is `app_data_dir()/cleanup-log.jsonl`; I did not check where that resolves on this machine. The log has no size limit or rotation.

Done (todo 4):
- `core/cleanup_plan.rs`: `plan_candidates` (dry run) and `execute_candidates`. Each candidate is re-checked on disk at plan time and again at execute time: the path must resolve to the same place, still be the same kind of item and not a symlink, have the same modification time as when it was found, and pass the guardrails. A failed check lists the item as excluded (plan) or failed (execute) and it is not moved. It reuses `PlanItem`, `CleanupPlan` and `execute` from sub-task 7, so the Trash call and per-item guardrail check are the existing, tested code. In this flow `PlanItem.node` holds the candidate id.
- `commands/cleanup_scan.rs`: `discover_cleanup`, `cancel_discovery`, `plan_cleanup_candidates`, `execute_cleanup_candidates`, registered in `lib.rs`, with separate `CleanupScanState`. The UI sends only candidate ids. A plan is single use and is invalidated by a newer plan, a newer discovery, or a second execute. Execution is refused while a search runs. Moved or already-gone items leave the stored list; failed ones stay.
- Items outside the home folder still need the acknowledgement, as in sub-task 7.
- Name exclusions now take a list, and the app passes both its bundle identifier and its package name (`carbon-treemap-disk`), so the dev-build cache folder is excluded by name, not only by the recent-use window. This resolves the note from todo 3.
- Tests: 14 in `cleanup_plan`, 14 in `cleanup_scan`. Full suite 188 passed, 4 ignored; `cargo clippy --all-targets -D warnings` and `cargo fmt --check` clean. The Trash is faked in these tests; the real `MacTrashProvider` was not run for this flow.
- Not run: nothing has called the new commands from the running app. There is no TypeScript wrapper yet (that is todo 6 and 7).
- Mistake caught on the way: one of my new tests deadlocked (a lock guard in an argument lived while the callee took the same lock). It hung the test run for over a minute. Fixed by cloning first. The production code does not hold two of these locks at once in that order; this was a test-only bug.

Done (todo 3):
- `core/cleanup_discovery.rs`: `discover(ctx, cancel, on_progress)` returns `Discovery { candidates, skipped, excluded }`. It uses the `Scanner` and `SizeAccounting` traits, so it stays OS-agnostic. It performs no writes. A symlinked root or child is skipped, and a candidate whose resolved path leaves its root is dropped. Each skip carries a reason (`recent`, `unreadable`, `symlink`, `outsideRoot`). Candidates are sorted largest first and numbered from 1; the ids are only valid for the discovery that produced them.
- Cancellation is checked before each candidate and passed into the scanner. Progress is reported once per candidate, not per file.
- 13 new tests on generated fixtures: sizes and file counts, ordering and ids, name exclusions, recent window, symlinked child and root, missing root, whole root, unreadable folder, cancel, no writes, progress, JSON shape. Full suite: 160 passed, 4 ignored. `cargo clippy --all-targets -D warnings` and `cargo fmt --check` are clean.
- Source search: every `remove_file` / `remove_dir_all` / `remove_dir` call is in test code. No permanent-delete call exists in production code.
- The tests use a fake scanner that sums plain files. The real `MacScanner` was exercised only by the ignored test below.

Real-machine run (`cargo test --release real_home -- --ignored --nocapture`, read-only, one run, warm cache):
- 0.81 s. 131 candidates, 23.58 GB total, 119 entries excluded by name, 11 skipped as recent (all `Recent`; none unreadable, symlink or outside the root).
- Largest: DerivedData `BudgetPlanner-...` 4.3 GB, `Caches/Adobe` 4.0 GB, `.npm/_cacache` 3.0 GB, `Caches/Homebrew` 2.2 GB, `Caches/Google` 2.2 GB.
- The 24-hour window skipped `GeoServices`, `ollama`, this app's own `carbon-treemap-disk` cache and similar, plus 5 log folders. This is the intended effect.
- The total (23.58 GB) is higher than my earlier `du` figures for the same folders added together (roughly 13.1 + 0.8 + 6.9 + 2.8 = 23.6 GB before exclusions and skips, then fewer after). The sizes use different rules (`MacScanner` counts allocated bytes and each hard link once; `du -sk` was run once per folder), so I do not treat either as exact. They are close; I did not reconcile them.
- Note: `~/Library/Caches/carbon-treemap-disk` was skipped only because it was recent, not by name. It is named after the binary, so it is probably a cache of this app's dev build, but I did not confirm that. The name rule and the guardrails use the bundle identifier (`com.carbon.treemap`), which does not match this folder name. Todo 4 must decide whether to also exclude the binary name.
- DerivedData's shared folders (`ModuleCache.noindex` 777 MB, `SDKExplicitPrecompiledModules` 539 MB) are offered as ordinary candidates. Left as is; flagged for your decision.

Done (todos 1 and 2):
- `core/cleanup_catalog.rs`: `Category`, `CatalogRoot` (`Children` or `Whole`), `NameRules`, `is_recent`, `stays_inside`, `RECENT_WINDOW` (24 h). Pure functions, no I/O. `platform/macos/catalog.rs`: `catalog_roots(home)` and `name_rules(app_identifier)`. Nothing calls them yet (`#[allow(unused_imports)]` on the re-export; remove it when discovery uses them).
- Checks that ran: `cargo test` (147 passed, 3 ignored; 15 are new), `cargo clippy -D warnings`, `cargo fmt --check`. No frontend change yet.

Measured on this machine (one machine, one point in time):

| Root | Exists | Size (`du`) | Children | Recent (< 24 h) | Notes |
|---|---|---|---|---|---|
| `~/Library/Caches` | yes | 13.1 GB | 220 (21 are plain files, 0 symlinks) | 20 | 116 are `com.apple.*`, about 1.5 GB |
| `~/Library/Logs` | yes | 0.8 GB | 31 (5 plain files, 0 symlinks) | 5 | |
| `~/Library/Developer/Xcode/DerivedData` | yes | 6.9 GB | 9 (0 symlinks) | 0 | Mixes project folders with shared ones such as `ModuleCache.noindex` |
| `~/.npm/_cacache` | yes | 2.8 GB | offered whole | n/a | Last modified about 4,100 hours ago |

- The 24-hour window hides little here: 20 of 220 cache entries and 5 of 31 logs, and by size only about 160 MB (`GeoServices`). It may hide more on a machine in heavy use.
- Two `du` runs on `~/Library/Caches` disagree: 13.1 GB for the folder, but 15.4 GB for the non-Apple children plus 1.5 GB for the Apple ones. I did not find out why. Discovery sizes with `MacScanner`, not `du`, so compare once discovery exists. Treat all sizes above as approximate.
- Not in the catalog yet: pnpm (`~/Library/pnpm` exists here) and the cargo registry (`~/.cargo/registry` exists here). How they share or link files is unchecked.
- DerivedData holds shared folders (`ModuleCache.noindex` and similar) beside per-project ones. Xcode regenerates them, but whether to offer them as separate candidates is undecided.
- `~/Library/Caches` has plain files at the top level, so a candidate can be a file or a folder. Discovery must not assume folders.

---
### 10. Duplicate finder and cleanup
**Intent:** Find files with identical content, show how much space they waste, and let the user move the extra copies to the Trash. It scans the user's home folder by default, and the user can narrow it to a chosen folder. Safety comes first: the feature must never remove the last copy of a file, and it must not offer anything it cannot prove is identical.

**Status:** [-] in progress. Todos 1 to 10 done; todos 11 and 12 not started. This promotes the "Duplicate detection and duplicate cleanup" item from Future Phases (decided by user).

**Next (todo 11, in this order).** Everything in code is built and checked by tests, but the screen has never been rendered and no command has run inside the app. Step 1 is therefore a look, not a test of safety. Every step needs a person at the running app; I cannot do them from the command line.
1. **You: launch the app and look** (`npm run tauri dev`). Open "Find duplicates" from the header and the side nav. Check the start screen, the two tiles, the Options accordion and the folder pickers in both themes. Fix any layout problem first; the places most likely to need it are listed under "Done (todo 8)". Also run `npm run lint` and `prettier --check` once more: the `SelectionActionBar.tsx` lint error and the Prettier warnings on `App.tsx` and `app.scss` were there before this work and are still open.
2. **Make a small test folder** with known contents, for example `~/duplicate-test` holding one 2 MB file copied three times under different names and folders, one different file of the same size, and one tiny file. Scan that folder only. Expect one group of three, the different file not grouped, and the tiny file ignored. Confirm the progress screen appears and Cancel returns to the start screen.
3. **Dry run:** "Select suggested duplicates", then "Review and move to Trash". Read the preview: each group shows what stays and what moves, and exactly one copy stays. Press Cancel. Confirm nothing moved on disk and the selection is still there. Then try to tick every copy by hand; the last unticked box must stay disabled.
4. **Protected folder:** repeat with one folder protected. Its copy must show "Protected", have no usable checkbox, and survive the suggestion.
5. **One small real move:** move one copy from the test folder. Check that the file is in the Trash, the other two copies are untouched, `cleanup-log.jsonl` in the app data folder has an entry with the original path, the group shrinks to two copies and "Empty the Trash to free the space" is shown. Try Put Back in Finder and write down whether it works (the plan already lists this as a risk).
6. **Stale-state checks in the app:** change one copy on disk after the scan, then preview; it should be listed under "Not included". Start a second scan, then use an old preview; it should be refused.
7. **Home folder in the app:** run the default scan, watch the progress stages and the time, cancel one run part-way, and run it again to the end. Compare with the 2.55 s warm-cache command-line figure. Do not move anything from the home-folder results in this step.
8. **Then decide** whether hidden tool folders (`~/Games`, `.lmstudio`, `.ollama`, `.cache`, `.rustup`, `.cargo`) are skipped by default. It is better to decide after step 7 gives a real in-app time. If that time is slow, the same result decides whether the hash cache moves up in Phase B.
9. **Keyboard and VoiceOver pass** on the start screen, the results list and the confirm modal, using the unchecked list in "Done (todo 9)". Record what was and was not found.

When 1 to 9 are done, write "Done (todo 11)" here with what was seen, what was not, and any fixes. Until then this sub-task stays in progress.

**After todo 11, Phase B order (todo 12), each its own reviewed step:**
1. Per-group keeper change and "always keep copies in this folder". The Rust ranking already accepts pinned folders but the commands pass an empty list, so this is the only Phase B item with finished back-end work waiting on it.
2. Hash cache keyed by path, size, modification time and inode. Do it only if step 7 above shows a first scan is too slow. A mismatch must always mean "rehash".
3. Treemap entry point ("find duplicates in this folder").
4. File-type filters.
5. Frontend tests for `useDuplicates` and the IPC error mapping; the project has none for state or components today.

Done (todo 10): required tests checked against the list, one gap closed, real-home run measured.
- Each "Tests required" item was mapped to an existing test by reading the test names, then spot-checked in the code for cancellation. Covered: grouping and same-name/same-size cases (`core/duplicates.rs`); hard links; empty and small files; symlinks (`file_walker.rs`, `content_hasher.rs`); a file changed after the scan, at plan and at execute time; the last-copy rule, including a missing or changed kept copy; stale and unknown ids, a second execute, a new scan invalidating the plan; partial failure; guardrails; protected folders; deterministic ordering with ties.
- Gap found and closed: cancellation was tested before the start and during the sample stage, but never during the full-hash stage. Added `cancelling_during_the_full_hash_stage_stops_the_scan` (the scan stops after the first full read).
- Results: `cargo fmt --check` clean, clippy `-D warnings` clean, `cargo test` 371 passed, 6 ignored (370 before).
- Source search: no permanent-delete, write, create or rename call in the code before the `#[cfg(test)]` line of any of the seven new files. The only hits are `remove_files` (an in-memory list function, not a file operation) and `OpenOptions` in `content_hasher.rs`, which opens files read-only with `O_NOFOLLOW | O_NONBLOCK`.
- Real home folder, release build, read-only, warm cache (this run, the home folder had changed since todo 3): 7,631 files listed, 350 groups holding 806 files, 3.35 GB reclaimable, 6.37 GB hashed in full, 0 issues. End to end 2.55 s. Sample stage: 1,288 files in 0.37 s; 842 matched on size and sample, 806 confirmed by the full hash, 36 sample-only false positives (4.3%).
- Walk with exclusions off: 21,524 files and 10.1 s, 3 cloud folders skipped. With the 4 KiB minimum: 187,264 files in 1.29 s. The default result held in memory is about 1.3 MB.
- Memory: the test process peaked at 39 MB resident (36 MB footprint) by `/usr/bin/time -l`. That includes a second walk the report does for its false-positive count, so the scan alone is lower. It is a figure for this home folder only.
- Not verified: cold cache (every figure is a warm-cache run, so real first-run time is unknown and probably longer, since hashing 6 GB is read-bound); an external or spinning disk; iCloud placeholders; a much larger home folder; time through the running app including the progress events. The 2,500 MB/s figure is mostly the page cache.
- Still open for you: `~/Games` (144 GB), `~/.lmstudio` (28 GB), `~/.ollama` (20 GB), `~/.cache`, `~/.rustup` and `~/.cargo` are scanned. The warm-cache run above includes them and took 2.55 s, but I did not measure how much of the 6.37 GB hashed or the 806 grouped files comes from them, and I have no cold-cache time. I have not excluded them. Say if you want hidden folders skipped by default.

Done (todo 9): accessibility pass, by reading the code and Carbon's source. **Nothing was tested with VoiceOver, a keyboard, or an automated checker such as axe.** The project has no tool for that, and the screen has never been rendered.
- Fixed: a disabled checkbox cannot take focus, so its reason ("Protected folder: this copy is always kept", "At least one copy must stay") was only a tooltip. It is now part of the checkbox's accessible name as well.
- Fixed: when the scan state changes (start, finish, cancel, fail) the button that had focus is replaced and focus fell to the page. Focus now moves to the page heading, as on a view switch.
- Fixed: the scan result was not announced. A visually hidden `role="status"` region now says "Scan started", "Scan finished. Found N groups, about X that could be freed", "No duplicates found", or the error. It waits until the first page of groups has loaded so the numbers are right. The visible summary is no longer a live region, so it is not read twice.
- Fixed: Carbon's `ProgressBar` announces its helper text politely, and progress events arrive about every 100 ms. The helper text is now only the stage name, which changes three times per scan. The running counts and current path are in a plain paragraph that is not announced.
- Fixed: the group title in the accordion ran the name and the figures together for a screen reader. A space now separates them.
- Already in place from the earlier screens: the danger modal opens with focus on Cancel and returns focus to the heading when it closes; Carbon modals trap focus; every control is a real button, checkbox, select or link; the action bar is a polite live region; colour comes from Carbon tokens in both themes.
- Not checked: tab order across the start screen and the results; whether `RadioTile` is operable by arrow keys in a `TileGroup`; whether an accordion title with several spans reads sensibly; whether the visually hidden status region is read at all in WKWebView; contrast of the tags in the confirm modal (green, red, magenta, gray) in both themes; the screen at narrow widths and with large text; how a 1,000-row list behaves with a screen reader. Whether the tooltip on a disabled checkbox shows at all is also unchecked.
- Judgement calls: I made the group's accordion title carry the figures, so a screen reader hears them when it lands on the group; that makes the title long.

Done (todo 8):
- `components/DuplicatesView.tsx` (the screen) and `components/DuplicateConfirmModal.tsx` (the confirm flow). `useDuplicates` now also holds the start-screen options and a selection that tracks its own count and bytes.
- Start screen: two tiles, "Home folder (recommended)" (the default) and "A folder…" with a folder picker and the existing recent folders. "Protect a folder" is a visible section with an add button and a removable list. An "Options" accordion has the smallest file size (100 KB, 1 MB recommended, 10 MB, 100 MB), the five skip switches (all on) and the cloud-folder toggle (shares the saved setting with the treemap scan). "Find duplicates" is disabled until a folder is chosen when that tile is selected. A line under the button says the scan only reads and one copy always stays.
- Scanning: a progress bar with the stage in words (listing, comparing samples, confirming matches with bytes read), the current path, and Cancel. The bar has a percentage only when the total is known. A cancelled scan returns to the start screen with an info notice.
- Results: a summary line (groups and reclaimable space), scan notes (files listed, protected folders, hard links counted once, skipped cloud folders, issue list with the full count), "Select suggested duplicates", "Clear selection", sort (space to reclaim, largest files, most copies), a filter box, and expandable groups. Each group row shows the name, copy count, size, space to reclaim and how many copies are chosen. Each copy has a checkbox, folder, last-changed date, "Suggested to keep" and "Protected" tags, and Show in Finder. "Load more groups" appears when groups are not loaded yet. "New scan" returns to the start screen.
- The checkbox is disabled for a protected copy, and for the last copy that is not chosen, with a tooltip saying why. Rust still enforces both rules; the UI is only a convenience.
- The action bar reuses `SelectionActionBar` ("Review and move to Trash"). The confirm modal is a danger modal with initial focus on Cancel. It shows, per group, which copies stay (tagged) and which move, then anything not included and why, and the acknowledgement checkbox for files outside home. After a move it shows the counts, any failures with the reason, how many groups remain, and the "empty the Trash to free the space" note.
- Checks: `tsc --noEmit` clean, `vite build` succeeds, `vitest` 64 passed, Prettier clean on all new files.
- Not verified: the screen has never been rendered or clicked. Typecheck and build passing says nothing about layout, focus order or what the Carbon components actually do. Carbon's `Accordion` with a rich `title`, `RadioTile` inside `TileGroup`, and the inline `Select` in the toolbar are the places most likely to need a visual fix. The filter box only searches groups already loaded, and says so when none match. There are no component tests (the project has no component test setup). `npm run lint` still reports the earlier `SelectionActionBar.tsx` error, and Prettier still flags `src/App.tsx` and `src/styles/app.scss`; I did not fix them.
- Judgement calls to confirm: the size choices stop at four values rather than a free number field. The filter is client-side over loaded groups rather than a Rust search. Pinning a folder as "always keep" is still not in the UI.

Done (todo 7):
- `src/ipc/index.ts`: types for the scan request, summary, progress, groups page, files, picks, suggested picks, plan, and execute result, matching the camelCase output of `commands/duplicates.rs`. Wrappers: `startDuplicateScan`, `cancelDuplicateScan`, `getDuplicateGroups`, `suggestDuplicatePicks`, `planDuplicateCleanup`, `executeDuplicateCleanup`, `onDuplicateProgress`. All errors go through `toCleanupError`.
- `CleanupErrorKind` and `CLEANUP_ERROR_KINDS` now include `noDuplicateScan`, `duplicateScanRunning`, `lastCopy`, `protectedFile`, `invalidRoot`, `permissionDenied`. Before this, a Rust error of those kinds would have been shown as `other`.
- `src/state/useDuplicates.ts`: scan state, group listing with sort and "load more" (page size 50), selection as group id to set of file ids, `selectSuggested`, and the `begin` / `confirm` / `close` flow copied from `useCleanupScan`. Only ids are sent. A late page from an older request is ignored. A new scan clears the selection and the plan state. After a move it reloads as many groups as were on screen.
- `src/views.ts` has `'duplicates'`. "Find duplicates" is in the header menu and the side nav (the `Copy` icon). `App.tsx` renders `DuplicatesView` and a `SelectionActionBar` for it. `DuplicatesView.tsx` is a placeholder (heading and scan status only); the real screen is todo 8.
- Checks: `tsc --noEmit` is clean, Prettier is clean on the files I touched, and `vitest` reports 64 passed.
- Not verified: there are no tests for `useDuplicates` or the new IPC wrappers (the frontend has tests only for pure treemap code), and nothing was run in the real app. `npm run lint` reports one error, `'React' is not defined` in `SelectionActionBar.tsx` line 27, and `prettier --check` flags `src/App.tsx` and `src/styles/app.scss`. All of these were already there: I stashed my changes and got the same results. I left them alone. The `App.tsx` Prettier warning means I could not tell whether my own edits to that file are formatted correctly.
- Open: `CleanupConfirmModal` and `CleanupView` have `switch` statements on the error kind that do not know the new kinds. They fall to the default text. The duplicate screen will have its own confirm modal in todo 8, so I did not change them.

Done (todo 6):
- `commands/duplicates.rs`: `DuplicateState` (`active`, `found: Option<Found {groups, home}>`, `pending`, `generation`, `next_plan_id`) plus six commands registered in `lib.rs`: `start_duplicate_scan` (async, cancellable), `cancel_duplicate_scan`, `get_duplicate_groups`, `suggest_duplicate_picks`, `plan_duplicate_cleanup`, `execute_duplicate_cleanup`. The logic sits in plain functions (`resolve_params`, `run_scan`, `store_scan`, `groups_page`, `suggested_picks`, `make_plan`, `run_plan`) so tests do not need a Tauri app.
- `found` is an `Option`, so "no scan yet" (`noDuplicateScan`) differs from "scan found nothing" (empty groups).
- Scan request: root must be absolute, existing and a folder (canonicalised); minimum size defaults to 1 MiB with a floor of 4 KiB; protected roots are de-duplicated; a scan root equal to or inside a protected root is refused; an unreadable protected folder is refused rather than ignored.
- Paging: default 50 groups, cap 200; sort by `wastedSpace`, `fileSize` or `copyCount`. The summary lists at most 100 issues but sends the full count. `suggested_picks` stops at the 1,000-move plan cap and sets `truncated`.
- Plans are single-use, cleared by a newer scan, checked against the scan generation, and need the acknowledgement for paths outside home, as in `cleanup_scan`. After execute, `remove_files` (new in `core/duplicate_plan.rs`, 4 tests) shrinks groups; a group with fewer than two copies or no movable copy leaves the list. The log goes to `cleanup-log.jsonl` through `log_moved`, and `logSaved` is reported.
- Progress event `duplicate-progress`: `stage` (`listing`, `fingerprinting`, `hashing`), `done`, `total`, `bytes`, `currentPath`.
- `CleanupErrorKind` gained `NoDuplicateScan`, `DuplicateScanRunning`, `LastCopy`, `ProtectedFile`, `InvalidRoot`, `PermissionDenied`. `KeepReason` now derives `Serialize`. `LOG_FILE` in `cleanup_scan.rs` is `pub(super)`. New constants in `core/config.rs`: `MIN_DUPLICATE_MIN_SIZE_BYTES`, `MAX_DUPLICATE_GROUPS_PER_PAGE`, `MAX_DUPLICATE_ISSUES_SHOWN`, `MAX_DUPLICATE_MOVES_PER_PLAN`.
- 45 new tests in `commands/duplicates.rs`. `cargo fmt --check` and clippy `-D warnings` are clean; `cargo test` reported 370 passed, 6 ignored (321 before).
- Mutation checks, done by hand and reverted (file diffed identical): ignoring the generation made 1 test fail; not clearing the pending plan on a new scan made 1 fail; not consuming the plan made 1 fail.
- Source search: `remove_file`, `remove_dir` and `fs::write` appear in `commands/duplicates.rs` only after the `#[cfg(test)]` line (864).
- Judgement calls to confirm: the commands pass an empty pin list to `KeepPreferences`, so "always keep copies in this folder" is not reachable from the UI yet (Phase B). The new error kinds were added to the shared `CleanupErrorKind`, so todo 7 must extend `CLEANUP_ERROR_KINDS` in `src/ipc/index.ts`.
- Not verified: the Tauri commands have never been invoked from the running app. The real macOS Trash was not used (fake provider). The progress event was not observed end to end. `PermissionDenied` for a protected folder is untested.

Done (todo 5):
- `core/duplicate_plan.rs`: `plan_duplicates(groups, picks, guardrails)` builds a dry-run `DuplicatePlan` and performs no writes. `execute_duplicates(groups, plan, guardrails, provider)` moves the planned copies. It reuses `PlanItem`, `ItemResult`, `ExcludedItem`, `Guardrails` and `execute` (so `move_one` and its guardrail check) from `cleanup`. `PlanItem.node` and `ItemResult.node` hold the file id. `MAX_DUPLICATE_MOVES_PER_PLAN` (1,000, same as the other cleanup plans) is in `core/config.rs`.
- Input is a group id plus file ids per pick. All picks for one group are merged before the last-copy rule runs, so two picks that each leave a different copy cannot add up to every copy (tested). An unknown group, an unknown file, a protected file, a selection that moves every copy, or too many moves refuses the whole plan with a `PlanError`. Nothing is partly planned.
- Plan-time checks per copy to move: the path resolves to the same place, it is a regular file (a symlink or folder is refused), size and modification time match the scan, and the guardrails allow it. A failure puts that copy in `excluded` with a reason (`missing`, `changed`, or the guardrail reason). A copy that stays gets the same disk check without the guardrails; if none of the copies that stay still match, nothing in that group is planned.
- Execute-time checks, before every single move: the group is still in the scan; the planned moves still pass `check_selection` (so a forged plan that covers every copy, or names a protected copy, moves nothing); the file's id and path match the scan; a copy that stays is still on disk with the same size and time; the disk check and guardrails again; then `move_one`. If a kept copy vanishes part-way through a group, the rest of that group is not moved (tested). A failure on one file does not stop the others. A group that is refused does not stop other groups.
- A plan group lists the copies that stay (`kept`) so the confirmation can show them, and the copies to move, largest total first.
- 34 new tests on temp-folder fixtures with a fake trash. `cargo test` reported 321 passed, 6 ignored (287 passed, 6 ignored before). `cargo fmt --check` and clippy `-D warnings` are clean.
- Mutation check, done by hand and then reverted (the file was diffed against a copy and is identical): removing the keeper check at execute time made 4 tests fail; making execute ignore a failed last-copy check made 2 tests fail. The first attempt at the second mutation did not compile, so it was redone another way.
- Source search: no `remove_file`, `remove_dir` or `fs::write` before the `#[cfg(test)]` module in `duplicate_plan.rs`.
- Not done here, left for todo 6: single-use plan storage, invalidation by a new scan, the acknowledgement for paths outside home, and writing the log. The pure functions do not know about any of them.
- Not verified: it has never run against the real macOS Trash; the fake provider stands in. Only whole seconds of modification time are compared, so a file edited within the same second and left the same size passes the check. A copy that is a hard link to a kept copy would have been collapsed by the scan, so it cannot appear here, but I did not test a hard link created after the scan. The group-ordering test uses two temp roots and a guardrail with no home folder, which is not how the app will call it.

Done (todo 4):
- `core/duplicate_keep.rs`: pure functions, no filesystem access. `suggest(group, prefs)` returns the keeper, the copies that can go with it, and the `KeepReason` that decided it (with plain-words text for the UI). `check_selection(group, move_ids)` is the last-copy rule. `moves_keeping(group, keeper)` gives the moves for a keeper the user chose. `KeepPreferences::new(home, pinned)` carries the "always keep copies in this folder" pins.
- Ranking order, first rule that differs decides: protected copy, pinned folder, not in Downloads/Desktop/Trash, not a copy-style name, oldest modification time (an unknown time counts as newest), shortest path, file name, full path. The order does not depend on the order of the files in the group (tested by rotating and reversing).
- Copy-style names recognised: `name (N)` with 1 to 3 digits, `name copy`, `name copy N`, `name - copy`, `copy of name`. `name (2024)`, `name 2`, `copyright` and `photocopy` are not. This is a judgement-based list and was not checked against real file names.
- "Poor location": `~/Downloads` and `~/Desktop` (needs the home folder passed in) and any folder named `.Trash` or `.Trashes` at any depth. Matching is by whole path components and case-insensitive, so `Downloads-archive` does not match `Downloads`.
- Last-copy rule: every id must belong to the group, none may be protected, and at least one copy must stay. Repeated ids count once. A test runs every subset of a four-file group (one protected) and checks that none empties the group and none moves the protected file.
- A suggestion is never a protected file in `moves`, and always passes `check_selection` (tested).
- `fold` in `duplicates.rs` became `pub(super)` so the ranking uses the same case folding as protection. `is_under` stays private.
- 31 new tests. `cargo test` reported 287 passed, 6 ignored (256 passed, 6 ignored before). `cargo fmt --check` and clippy `-D warnings` are clean.
- Not verified: the ranking has not been run over real groups, so how well the suggestion matches what you would pick is untested. The Downloads/Desktop rule uses the home folder passed in, so a scan of another user's or another volume's folder only gets the Trash rule. A pinned folder that is a symlink path is compared by text, not resolved.

Done (todo 3):
- `platform/macos/content_hasher.rs`: `MacContentHasher` implements `ContentHasher` with BLAKE3 (`blake3` 1.x, default features only: no `rayon`, no `mmap`). Hashing is single-threaded per file because the engine already hashes several files at once.
- Files are opened with `O_NOFOLLOW | O_NONBLOCK` and must be regular files. A symlink, FIFO, device or folder that appeared after the walk is refused (`InvalidInput`, or an open error for a symlink) instead of followed or waited on.
- Partial fingerprint: the size plus the first and last 64 KiB. Files up to 128 KiB are fingerprinted whole. Full hash: 1 MiB reads, cancel checked before each read, `ErrorKind::Interrupted` on cancel.
- A file whose length differs from the walked size, or that shrinks or grows during the read, fails with `InvalidData`, which the engine already reports as a file that changed.
- 14 new tests: equal content, hash equals `blake3::hash` of the bytes (3 chunks plus a remainder), different last byte, partial sees a change in the first or last block, partial misses a middle change and the full hash catches it, small files hashed whole, cancel, missing file, permission denied, symlink refused, FIFO refused without blocking, folder refused, stale size, and shrink/grow detection. `cargo test` reported 256 passed, 6 ignored (242 passed, 5 ignored before). `cargo fmt --check` and clippy `-D warnings` are clean.
- Source search: the new file opens files read-only and contains no `remove_file`, `write` to a user file or `rename` outside `#[cfg(test)]`.
- Release binary: 4,523,376 bytes with BLAKE3. The earlier figure was recorded as "4.4 MB" without an exact byte count, so the increase is about 0.1 MB at most and not precisely measured.

Real-machine run on `$HOME` (`cargo test --release real_home_hash -- --ignored --nocapture`, read-only, default options, one run, warm cache):
- End to end: 2.46 s. 6,966 files listed, 339 groups holding 779 files, 3.29 GB reclaimable, 6.24 GB read by the full hash, 0 issues.
- The partial stage, re-run on its own: 1,252 files sampled in 0.37 s. 807 matched on size and partial fingerprint; 779 were confirmed by the full hash. So about 28 files (3.5% of the partial matches) were partial-only false positives that the full hash removed. This confirms the partial stage is a filter, not proof.
- The "28" subtracts the engine's group file count from a second walk's partial-match count. The two walks ran seconds apart, so a file changing between them would shift it by a few. Treat it as approximate.
- The 2,542 MB/s figure is bytes hashed divided by total elapsed time on a warm page cache. It is not disk throughput. A cold cache, an external disk and a network volume were not tried.
- Decision still open: the large hidden tool folders (`~/Games`, `~/.lmstudio`, `~/.ollama`, `~/.cache`, `~/.rustup`, `~/.cargo`) are walked and hashed. At 2.46 s warm the cost does not argue for excluding them, but this run does not show a cold-cache cost. Keep as is until you decide.
- Not verified: behaviour on a file changing mid-read in a real running system (covered only by a fixture that passes a wrong expected length); `O_NONBLOCK` on network filesystems; files with extended attributes or compression flags other than `SF_DATALESS`; the NEON path was not checked separately from the default build.

Done (todo 2):
- `core/duplicate_exclusions.rs`: pure rules, no filesystem access. `Exclusions` has one switch per kind: packages (`.app`, `.photoslibrary`, `.xcarchive`, `.framework`, `.xcodeproj` and similar), `node_modules`, `.git`, `~/Library` and `~/.Trash`. The `Library` and `.Trash` rules apply only directly under the home folder. A rule applies to folders below the chosen root, never to the root itself: choosing `~/Library/Caches` or a folder named `node_modules` walks it as asked.
- `platform/macos/file_walker.rs`: `MacFileWalker` implements `FileWalker`. Same design as `MacScanner` (workers read one directory each, one collector). It keeps every regular file at or above the minimum size, skips symlinks, sockets and devices, stays on the root's volume, skips excluded folders and cloud-sync folders (reported, opt-in to include), and skips "dataless" cloud placeholders (`SF_DATALESS`) so a later hash cannot trigger a download. The walker never opens file contents.
- `platform/macos/bulk.rs`: new `read_dir_bulk_files` also returns the regular-file flag, logical size, modification time and `st_flags`. `read_dir_bulk` (used by the treemap scan) asks for exactly the same attributes as before, so the treemap path is unchanged; the existing scanner tests still pass. A portable `readdir` + `lstat` fallback is used where `getattrlistbulk` is unsupported.
- `DuplicateOptions` gained `exclusions`.
- 26 new tests on generated fixtures (9 for the rules, 17 for the walker): size filter, metadata, symlinks, hard links, each exclusion, root-named-like-an-exclusion, home-relative rules, cloud skip/include/root-inside-mount, unreadable folder, bad roots, cancel, progress, and bulk-versus-portable agreement. `cargo test` reported 242 passed, 5 ignored (216 passed, 4 ignored before). Clippy `-D warnings` and `cargo fmt --check` are clean.
- Source search: no `remove_file`, `remove_dir_all` or `remove_dir` in the new files outside the `#[cfg(test)]` module.

Real-machine walk of `$HOME` (`cargo test --release real_home_walk -- --ignored --nocapture`, read-only, lists files only, one run each, warm cache):

| Setting | Files | Distinct inodes | Total | Time |
|---|---|---|---|---|
| default (1 MiB, exclusions on) | 6,773 | 5,934 | 230.71 GB | 0.60 s |
| exclusions off | 20,615 | 18,293 | 343.01 GB | 8.48 s |
| 4 KiB minimum, exclusions on | 172,897 | 156,929 | 240.50 GB | 1.11 s |

- With the defaults there were 0 issues and 0 cloud folders skipped, because `~/Library` (where the cloud mounts live) is excluded first. With exclusions off, 3 cloud folders were skipped and 1 issue was reported; I did not look at that issue.
- Excluding `~/Library`, packages, `node_modules`, `.git` and `.Trash` removes about 112 GB and about 14,000 files at the 1 MiB minimum. `~/Library` is du 216 GB on this machine, so it is the largest of those; I did not split the difference between the five rules.
- Hard links: 839 of the 6,773 default paths share an inode with another path. They would be collapsed, not reported as duplicates.
- Result held in memory at the default minimum: about 1.1 MB (computed from path lengths plus struct size, not measured with a profiler). At a 4 KiB minimum the file count is 25 times larger; I did not compute that memory figure.
- Survey on this machine before writing the rules: `~/.Trash` exists and is empty; `~/Library` has `CloudStorage` and `Mobile Documents`; outside `~/Library` there are 55 `node_modules` folders, 37 `.git` folders and 15 package folders (7 `.app`, 3 `.xcodeproj`, 3 `.musiclibrary`, 1 `.photoslibrary`, 1 `.imovielibrary`).
- Large non-hidden and hidden folders are NOT excluded: `~/Games` (144 GB), `~/.lmstudio` (28 GB), `~/.ollama` (20 GB), `~/.cache`, `~/.rustup`, `~/.cargo`. Only `.git` and `.Trash` are excluded among hidden folders. These will be walked, and their large files with a matching size will be fully hashed in todo 3. Whether to exclude hidden folders, or these specific tool folders, is a decision for you; the plan's earlier "include hidden" option is not implemented.
- Not verified: the dataless skip has no test (a placeholder cannot be created in a fixture); the `ENOTSUP` fallback to the portable reader is not exercised; the two read modes were compared on a small fixture only, not on the real home folder; external and network volumes and a cold cache were not tried; the packages list and the `st_dev` comparison (low 32 bits, as in the scanner) were not checked against volumes other than the boot volume.

Done (todo 1):
- `core/duplicates.rs`: `find_duplicates(walker, hasher, request, cancel, on_progress)` runs the pipeline: list, collapse hard links, group by size, partial fingerprint, full hash. It is read-only. Types: `DuplicateOptions`, `WalkedFile`, `WalkOutput`, `DuplicateGroup`, `DuplicateFile`, `DuplicateScan`, `DuplicateProgress`, `Stage`, `DuplicateError`.
- Traits: `FileWalker` in `platform/mod.rs` and `ContentHasher` in `core/duplicates.rs`. Neither has a real implementation yet (todos 2 and 3). `DEFAULT_DUPLICATE_MIN_SIZE_BYTES` (1 MiB) is in `core/config.rs`.
- Reference-folder mode is in the engine: files under a protected root are flagged `protected`. A group made only of protected files is dropped. A hard link to a protected file keeps the protected flag. `reclaimable_files` counts every unprotected file when a protected copy exists, and all but one otherwise.
- Behaviour to know about: an unreadable file is recorded as an issue and leaves its group (a group of three with one unreadable becomes a group of two). An unreadable root, including a protected one, fails the whole scan, so a protected folder can never be silently unprotected. Empty files are always ignored, even with `min_size` 0.
- Group and file ids are numbered from 1 and ordered by reclaimable bytes, then size, then path, so they are the same for any thread count (tested at 1 and 4 threads).
- 23 tests with a fake walker and a fake hasher. `cargo test` reported 216 passed, 4 ignored (193 before). `cargo clippy --all-targets -D warnings` and `cargo fmt --check` are clean.
- Not done or not verified: nothing reads real files yet, so nothing here has run on a real disk. Progress for the listing stage depends on the walker. Memory use is not measured; `find_duplicates` holds every listed path as a `PathBuf`, which will matter on a large home folder (see "Performance and memory"). Several "Tests required" items belong to later todos (plan, execute, last-copy rule, IPC) and are not covered. The paths in a group are compared lower-cased for protection only; two paths that differ only by case are otherwise treated as different files.

**Scope**
- **In scope:** exact duplicates only (same bytes). Home folder by default; a user-chosen folder; a reference (protected) folder; Move to Trash only.
- **Not in scope (see "Later"):** similar or near-duplicate images, duplicate-folder grouping, replacing copies with hard links or clones, permanent delete.

**How it fits the existing code**

| Existing piece | Reuse |
|---|---|
| `Scanner` trait / `MacScanner` | **Not reusable as is.** It builds the tree and folds files under 1 MiB into counters, so individual small files are gone. The finder needs its own file walker (`FileWalker` trait in `platform/`, macOS impl reusing the `getattrlistbulk` reader code). |
| `ScanOptions.include_cloud_folders`, `platform/macos/cloud.rs` | Reuse. Cloud folders stay skipped by default, for the same latency reason as sub-task 8. |
| `Guardrails`, `move_one`, `TrashProvider`, `execute`, `cleanup_log` | Reuse unchanged. No new Trash code, and still no hard delete anywhere. |
| `CleanupPlan` / `PlanItem` | Reuse for the preview. In this flow `PlanItem.node` holds a file id, as it holds a candidate id in sub-task 9. |
| Plan model from sub-tasks 7 and 9 (Rust owns the plan, UI sends ids, single use, invalidated by a newer run, re-checked at execute) | Same model. |
| `ActiveGuard` / `CancelToken` / progress events | Same pattern, with its own `DuplicateState`. |
| `AppView`, header menu, `SelectionActionBar`, `CleanupConfirmModal` | Extend `AppView` with `'duplicates'`; same nav pattern; same modal pattern (danger modal, initial focus on Cancel). |
| `ScanPanel` recent roots (`recentRoots`) | Reuse for the folder choice. |

**Detection pipeline (`core/duplicates.rs`, OS-agnostic)**
1. **Walk.** Collect regular files at or above the minimum size. Record `(size, device, inode, path)`. Do not follow symlinks. Skip cloud folders, other volumes (`cross_volumes = false`) and the default exclusions below.
2. **Same inode is not a duplicate.** Hard links to one inode are one file. Collapse them before grouping, and report them as "already shared" rather than as waste.
3. **Group by exact size.** Drop every file whose size is unique. This removes most of the disk with no file reads.
4. **Partial fingerprint.** Hash the first and last blocks of each remaining file (block size is a constant, to be tuned). Drop unique fingerprints.
5. **Full hash.** Hash the whole content of what is left with a fast cryptographic hash. BLAKE3 is the leading candidate because it is fast and multi-threaded, but **verify the crate and its current API before adding it**.
6. **Optional byte compare at plan time.** Before anything moves, re-read the kept copy and each copy to be moved and confirm they still match. A hash collision or a file edited since the scan then cannot cause a loss.

Each stage reports progress (files listed, files compared, bytes hashed) and honours the `CancelToken`. Groups are emitted as they are confirmed, so the user can start reviewing before the scan ends.

**Scope selection (the user's requirement)**
- **Default: the user's home folder (decided by user).** `/` and other volumes are not scanned by default, so no system-area exclusion list is needed. A folder outside home can still be chosen with the picker. Full Disk Access still matters for protected folders inside home; reuse `FullDiskAccessNotice` and show how many paths were unreadable.
- **Choose a folder.** Folder picker (`pickFolder`), recent folders (reuse `recentRoots`, with a separate list so it does not mix with treemap roots).
- **From the treemap.** "Find duplicates in this folder" when one folder is selected in the treemap action bar. This is the quickest way to scan a target folder.
- **Reference folder (first release, decided by user).** Pick a "protected" folder and a "scan" folder. Only files in the scan folder can be moved; the protected folder is never touched. Answers "which of these downloads do I already have in my library?" and is the safest mode.

**Defaults that keep the result useful and the scan fast**
- **Minimum size: 1 MiB** (matches the treemap's small-file threshold, and keeps the file list small). Choices in the UI: any size above 4 KiB, 100 KiB, 1 MiB, 10 MiB, 100 MiB. Zero-byte files are never grouped.
- **Excluded by default, each with an "include" switch:** application bundles and other package folders (`.app`, `.photoslibrary`, `.xcarchive` and similar), `node_modules`, `.git`, `~/Library` (its contents are mostly app-owned), and the Trash. Duplicates inside these are almost always intentional or app-managed, and removing them breaks things. The list is judgement-based; revisit with real data.
- **Cloud-synced folders: skipped**, with the existing toggle.
- **File-type filter** (display only, after detection): Images, Video, Audio, Documents, Archives, Other.

**Safety model**
1. **The UI never sends a path.** Rust holds the groups with ids. The UI sends back, per group, the ids to move.
2. **The last copy cannot go.** Planning rejects any selection that moves every member of a group, and rejects a group whose kept member no longer exists or no longer matches. Enforced in Rust, not only in the UI.
3. **Trash only.** The existing invariants hold. Source search for `remove_file`, `remove_dir_all` and `remove_dir` outside tests must stay clean.
4. **Re-check at execute time:** resolved path, still a regular file, size and modification time unchanged since the scan, guardrails (sub-task 7), and the kept copy still present and matching.
5. **Single-use plan,** invalidated by a new duplicate scan, a second execute, or a tree rescan, same as sub-tasks 7 and 9.
6. **Honest numbers.** "Wasted space" is `(copies - 1) x size` per group. Moving to the Trash frees nothing until the Trash is emptied, and APFS clones share blocks, so removing a clone may free little or nothing. Say so in the preview.

**Keep-one rules (which copy survives)**
Rust ranks the members of each group and marks one as the suggested keeper. The rule is shown to the user in plain words.
- Prefer the copy in a "real" location over `Downloads`, `Desktop`, the Trash, or a path with a copy-style name such as `file (1).pdf` or `copy of`.
- Then prefer the oldest modification date (the original), then the shortest path, then the name sorted first.
- The user can change the keeper per group, or set "Always keep copies in this folder" for a whole run. That pin is a priority for the ranking, and it can override the suggestion only inside the user's own choice, never the last-copy rule.

**Selection and cleanup flow**
- **Selection starts empty (decided by user).** A single button, "Select suggested duplicates", fills the selection from the keep-one rules, and the user can review every row before confirming.
- The confirm modal lists each group with the copy being kept and the copies being moved, the total, and the Trash note. A group that contains a path outside the home folder needs the existing acknowledgement.
- Per-item results (moved, already gone, failed with reason), then written to `cleanup-log.jsonl` like sub-task 9.
- After a move, groups shrink in place. A group left with a single member disappears. The summary updates ("2.1 GB moved, 1.3 GB still reclaimable"). No full rescan is needed.

**IPC design**
- `start_duplicate_scan({ root | null, options })` is async and cancellable. `null` means the home folder. Progress arrives as `duplicate-progress` events with the stage, counts and bytes. `cancel_duplicate_scan`.
- `get_duplicate_groups({ sort, filter, offset, limit })` returns one page. The group list can be huge, so the UI never receives it all at once (same idea as the bounded `get_view`). Cap the page size as a constant, as `MAX_VIEW_NODES` does.
- `plan_duplicate_cleanup({ picks: [{ group, files }] })` and `execute_duplicate_cleanup({ planId, acknowledged })`. Error kinds follow `CleanupErrorKind`, plus `lastCopy` and `noDuplicateScan`.
- Types are added to `src/ipc/index.ts`, written from the Rust structs and checked against a live call.

**UI**
- New view `'duplicates'`. Header: "Treemap", then a "Tools" menu with "Clean up caches" and "Find duplicates" (mirrored in the side nav, as in sub-task 9).
- **Start screen:** two Carbon selectable tiles, "Home folder (recommended)" and "A folder...", an optional "Protect a folder" picker (reference mode), recent folders, and an expandable "Options" section (minimum size, include hidden and package folders, cloud folders). A line says how long a home-folder scan usually takes, once measured.
- **While scanning:** a staged progress bar ("Listing files", "Comparing sizes", "Checking content"), counts, and a Cancel button. Groups appear as they are confirmed.
- **Results:** a summary line ("12.4 GB can be reclaimed in 3,204 groups"), sort (wasted space, file size, copy count), filter by file type and by folder, and a search box. Each group is an expandable row. Each file row has a checkbox, the folder path with the file name emphasised, modified date, a "Suggested to keep" tag, and "Show in Finder". A keyboard-reachable list; every action is a real control.
- **Delight, kept simple:** a "Reclaimable" figure that counts down as the selection grows; per-type breakdown bar; an empty state that says "No duplicates found" and what was searched; "Open in Finder" and, if feasible, Quick Look, so the user can look before deleting.
- Carbon components to check against the installed version before use: `TileGroup`/`RadioTile`, `ExpandableTile` or `TableExpandRow`, `ProgressBar`, `Tag`, `Search`, `Dropdown`.
- Accessibility follows sub-tasks 8 and 9: focus to the page heading on view switch, focus back to the review button after the modal closes, live regions for progress and results, contrast in both themes.

**Performance and memory (to be measured, not assumed)**
- A home-folder file list can be large. Store paths as a parent-directory arena plus names, as the tree does, and drop unique-size files early. Record peak footprint on a real home folder.
- Hashing is read-bound. Parallelism that helps an SSD can hurt a spinning or external disk; the thread count is a constant to tune, as sub-task 3 did.
- A persistent hash cache keyed by `(path, size, modified time, inode)` in the app data directory would make repeat scans fast. This is phase B, and it must treat a mismatch as "rehash", never as "identical".
- Reading file contents may trigger the download of cloud placeholders. Skip any file with the `dataless` flag, as sub-task 8 observed that flag in Box.

**Phases**
- **Phase A (the feature, usable end to end):** engine pipeline, home folder and chosen folder, reference-folder mode, exclusions and minimum size, paged group list, keep-one rule, last-copy rule, plan and execute through Trash, log, start screen, progress, results, confirm modal.
- **Phase B (the polish):** treemap entry point, type filters and search, per-group keeper change and "always keep in this folder", hash cache, Show in Finder, selection counter.
- **Later (needs separate decisions):** duplicate folders (a folder whose files all exist elsewhere); near-duplicate images; replacing a copy with an APFS clone or hard link so both paths stay and space is reclaimed. That last one changes files in place and breaks the "Trash only" rule, so it needs an explicit decision. Detecting existing APFS clones (to avoid promising space that will not come back) may be possible through the extended clone-id attribute of `getattrlist`; this is unverified and needs a check on a real volume.

**Todo List**
1. [x] Write the pipeline in `core/duplicates.rs` against a `FileWalker` trait and a hash function trait, with fake implementations for tests (no I/O in the grouping rules).
2. [x] Implement the macOS `FileWalker` (reuse the bulk reader) and the exclusion rules, with the home folder as the default root. Check each excluded path on a real machine and record it here.
3. [x] Add the hash dependency after verifying the crate and its current API. Implement partial and full hashing with cancellation and throttled progress.
4. [x] Implement keep-one ranking and the last-copy check as pure functions.
5. [x] Implement plan and execute for duplicate groups, reusing `move_one`, guardrails and the log. Single-use plans, item cap, re-check at execute.
6. [x] Add commands, register them in `lib.rs`, and add `DuplicateState`.
7. [x] Add the TypeScript wrappers, `useDuplicates` state and the `'duplicates'` view with the header and side-nav entries.
8. [x] Build the start screen, progress, results list and confirm modal (in code; never rendered).
9. [x] Accessibility pass for the new screens (code review only; nothing tested with a keyboard or screen reader).
10. [x] Run the tests below, then measure on a real home folder (warm cache only).
11. [ ] Manual run: dry run on a real folder, then one small real move. Record results here. See "Next" above for the steps.
12. [ ] Phase B items, each as its own reviewed step. See "Next" above for the order.

**Tests required before this is called done**
- Fixture trees: identical files in different folders are grouped; same size but different content are not; same name but different content are not.
- Hard links to one inode are one file, not a duplicate group.
- Empty files and files under the minimum size are never grouped. A symlink is never followed or grouped.
- A file that changes between hashing and plan time is rejected at plan time and again at execute time.
- Last-copy rule: a selection covering every member of a group is rejected, and so is a selection whose kept member has gone missing or changed.
- The UI cannot name a path: unknown or stale ids are rejected, a second execute of one plan is rejected, and a new scan invalidates the old plan.
- Partial failure leaves the other items moved and reports each result.
- Guardrails still block system paths and the home standard folders, and a protected path may be kept but never moved.
- Reference-folder mode: nothing in the protected folder is ever offered for moving.
- Cancellation stops each stage promptly and leaves no plan behind.
- Keep-one ordering is deterministic, including ties.
- A source search confirms no permanent-delete call outside tests.

**Risks and open questions**
- **A home-folder scan can still be slow and large.** `$HOME` had 313,450 tree nodes on the dev machine, and hashing reads file contents, which the treemap scan does not. Mitigations are the size filter, early dropping, cancellation and incremental results; none of it is measured yet. Cold-cache and external-volume behaviour is untested, as in sub-task 8.
- **Trash does not free space** (see sub-task 9). Same recommendation: keep the no-hard-delete rule and state it plainly. An "Empty Trash" step needs your explicit decision.
- **APFS clones.** Two files can have equal content and share storage, so removing one frees nothing. The preview may overstate the gain until clone detection is checked.
- **App-managed duplicates.** Files inside bundles, libraries (Photos, Music) and developer folders may be duplicated on purpose. The exclusions reduce the risk but the list is judgement-based.
- **Put Back.** Restoring may be manual for items moved with `NSFileManager` (sub-task 7). The log records original paths.
- **Hash choice and dependency.** Confirm the crate, its licence and its macOS build before adding it. This also changes the binary size; remeasure.
- **Files in use.** A file open in an app can change while being hashed. The size and modification-time re-check at plan and execute time is the safeguard; it is not a guarantee.
- **Cloud placeholders.** Reading `dataless` files could trigger downloads, so they are skipped. Not tested on iCloud.

**Decisions (user)**
1. Default scope: the home folder only, not the whole disk.
2. Pre-selection: nothing selected at the start. "Select suggested duplicates" is an explicit button.
3. Default minimum size: 1 MiB.
4. Reference-folder mode: in the first release.
5. Replacing duplicates with clones or hard links: not planned. Trash only. It stays under "Later" and needs a new decision before anyone builds it.

---



## Future Phases (not in this plan's scope)

- **Duplicate detection and duplicate cleanup:** now planned as sub-task 10 above.
- **Review-tier cache cleanup (deferred by user):** `node_modules` and Rust `target/` next to a manifest, old installers in Downloads, Xcode `iOS DeviceSupport`. Builds on sub-task 9.
- **Stale and unused file analysis:** needs OS-aware signals because last-access times differ in reliability across platforms.
- **Windows port:** implement the platform traits for Windows. Optional faster scan path that may need elevated rights.

## Implementation Order

Tasks 1 and 2 first, then 3, then 5 and 4, then 6, then 7, then 8. Each sub-task should be completed and reviewed before starting the next.

Sub-tasks 9 and 10 were added later and build on 7. Sub-task 10 is built end to end in code; its next step is the manual run in its "Next" block. Sub-task 9 still has its own open items (see its status line).
