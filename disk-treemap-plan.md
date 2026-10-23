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

## Future Phases (not in this plan's scope)

- **Duplicate detection:** re-walk files, group by size, then partial hash, then full hash. Uses the small files that MVP aggregates away.
- **Stale and unused file analysis:** needs OS-aware signals because last-access times differ in reliability across platforms.
- **Windows port:** implement the platform traits for Windows. Optional faster scan path that may need elevated rights.

## Implementation Order

Tasks 1 and 2 first, then 3, then 5 and 4, then 6, then 7, then 8. Each sub-task should be completed and reviewed before starting the next.
