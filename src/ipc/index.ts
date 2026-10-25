/**
 * Typed wrappers around Tauri IPC commands.
 * Each function maps 1-to-1 to a command declared in src-tauri/src/commands/.
 * Field names match the camelCase serde output on the Rust side.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { revealItemInDir } from '@tauri-apps/plugin-opener';

export type FullDiskAccessStatus = 'granted' | 'missing' | 'unknown';

export type ScanErrorKind =
  'cancelled' | 'permissionDenied' | 'notFound' | 'alreadyRunning' | 'other';

export interface ScanError {
  kind: ScanErrorKind;
  message: string;
}

export interface ScanIssue {
  path: string;
  kind: 'permissionDenied' | 'io';
  message: string;
}

export interface ScanSummary {
  totalBytes: number;
  fileCount: number;
  nodeCount: number;
  issues: ScanIssue[];
  /** Cloud-sync folders left out of the totals. */
  skippedCloudFolders: string[];
}

export interface ScanProgress {
  itemsScanned: number;
  bytes: number;
  currentPath: string;
}

export type ViewKind = 'dir' | 'file' | 'inaccessible' | 'other';

export interface ViewNode {
  /** Raw tree id. Absent for `other`. */
  id?: number;
  /** Empty for `other`; the UI supplies its own label. */
  name: string;
  kind: ViewKind;
  size: number;
  files: number;
  /** A directory with content that this view did not expand. */
  expandable: boolean;
  children?: ViewNode[];
}

export interface Crumb {
  id: number;
  name: string;
}

export interface ViewResponse {
  view: ViewNode;
  breadcrumbs: Crumb[];
}

export interface ViewRequest {
  /** Scan root when omitted. */
  node?: number;
  depth?: number;
  topK?: number;
}

export interface ViewError {
  kind: 'noScan' | 'unknownNode';
  message: string;
}

export type ReasonKind =
  | 'invalidPath'
  | 'scanRoot'
  | 'inaccessible'
  | 'missing'
  | 'systemPath'
  | 'homeFolder'
  | 'trash'
  | 'appData'
  | 'outsideHome'
  | 'changed';

export interface Reason {
  kind: ReasonKind;
  message: string;
}

export interface PlanItem {
  node: number;
  path: string;
  name: string;
  kind: 'dir' | 'file';
  /** Scanned bytes, not re-measured on disk. */
  size: number;
  files: number;
  warning?: Reason;
}

export interface ExcludedItem {
  node: number;
  path: string;
  name: string;
  reason: Reason;
}

export interface CleanupPlan {
  planId: number;
  /** Largest first. */
  items: PlanItem[];
  excluded: ExcludedItem[];
  /** Selected items left out because a selected folder already contains them. */
  merged: number;
  totalBytes: number;
  totalFiles: number;
  needsAcknowledgement: boolean;
}

export type CleanupOutcome = 'moved' | 'alreadyGone' | 'failed';

export interface CleanupItemResult {
  node: number;
  path: string;
  name: string;
  size: number;
  files: number;
  outcome: CleanupOutcome;
  message?: string;
}

export interface CleanupResult {
  results: CleanupItemResult[];
  moved: number;
  alreadyGone: number;
  failed: number;
  movedBytes: number;
  /** False if a newer scan replaced the tree while items were moving. */
  treeUpdated: boolean;
  totalBytes: number;
  fileCount: number;
}

export type CleanupErrorKind =
  | 'noScan'
  | 'unknownNode'
  | 'tooMany'
  | 'stalePlan'
  | 'needsAcknowledgement'
  | 'scanRunning'
  | 'noDiscovery'
  | 'unknownCandidate'
  | 'discoveryRunning'
  | 'cancelled'
  | 'noDuplicateScan'
  | 'duplicateScanRunning'
  | 'lastCopy'
  | 'protectedFile'
  | 'invalidRoot'
  | 'permissionDenied'
  | 'other';

export interface CleanupError {
  kind: CleanupErrorKind;
  message: string;
}

export type CandidateCategory = 'appCaches' | 'appLogs' | 'xcodeBuildCache' | 'npmCache';

export type SkipReason = 'recent' | 'unreadable' | 'symlink' | 'outsideRoot';

export interface Candidate {
  /** Valid only for the discovery that produced it. */
  id: number;
  category: CandidateCategory;
  path: string;
  name: string;
  kind: 'dir' | 'file';
  size: number;
  files: number;
  /** Seconds since the Unix epoch. */
  modifiedSecs: number;
  /** Some entries inside could not be read, so `size` is a lower bound. */
  incomplete: boolean;
}

export interface SkippedCandidate {
  category: CandidateCategory;
  path: string;
  name: string;
  reason: SkipReason;
}

export interface DiscoveryResult {
  /** Largest first. */
  candidates: Candidate[];
  skipped: SkippedCandidate[];
  /** Entries left out by name. They are not listed. */
  excluded: number;
  totalBytes: number;
}

export interface DiscoveryProgress {
  found: number;
  currentPath: string;
}

export interface CandidateCleanupResult {
  results: CleanupItemResult[];
  moved: number;
  alreadyGone: number;
  failed: number;
  movedBytes: number;
  /** False if the moved items could not be written to the cleanup log. */
  logSaved: boolean;
}

// --- duplicate finder ---

export interface DuplicateExclusions {
  /** App bundles and other packages. */
  packages: boolean;
  nodeModules: boolean;
  git: boolean;
  library: boolean;
  trash: boolean;
}

export interface DuplicateScanRequest {
  /** The home folder when omitted. */
  root?: string;
  /** Copies in these folders may be kept but are never moved. */
  protectedRoots: string[];
  minSizeBytes?: number;
  includeCloudFolders?: boolean;
  exclusions?: Partial<DuplicateExclusions>;
}

export interface DuplicateSummary {
  root: string;
  protectedRoots: string[];
  minSizeBytes: number;
  groupCount: number;
  reclaimableBytes: number;
  filesListed: number;
  bytesHashed: number;
  hardLinksCollapsed: number;
  /** The full count. `issues` lists only the first few. */
  issueCount: number;
  issues: ScanIssue[];
  skippedCloudFolders: string[];
}

export type DuplicateStage = 'listing' | 'fingerprinting' | 'hashing';

export interface DuplicateProgress {
  stage: DuplicateStage;
  done: number;
  /** Zero while the total is not known. */
  total: number;
  bytes: number;
  currentPath: string;
}

export type DuplicateSort = 'wastedSpace' | 'fileSize' | 'copyCount';

export type KeepReason =
  | 'protected'
  | 'pinned'
  | 'betterLocation'
  | 'originalName'
  | 'oldest'
  | 'shortestPath'
  | 'nameOrder';

export interface DuplicateFile {
  /** Valid only for the scan that produced it. */
  id: number;
  path: string;
  name: string;
  folder: string;
  /** Seconds since the Unix epoch. */
  modifiedSecs: number;
  /** In a protected folder. Can be kept, never moved. */
  protected: boolean;
  suggestedKeep: boolean;
}

export interface DuplicateGroup {
  id: number;
  /** Bytes per copy. */
  size: number;
  reclaimableBytes: number;
  reclaimableFiles: number;
  keepReason: KeepReason | null;
  /** Plain words for the suggested keeper. */
  keepReasonText: string | null;
  files: DuplicateFile[];
}

export interface DuplicateGroupsPage {
  groups: DuplicateGroup[];
  totalGroups: number;
  offset: number;
  reclaimableBytes: number;
}

export interface DuplicatePick {
  group: number;
  /** The copies to move. */
  files: number[];
}

export interface SuggestedPicks {
  picks: DuplicatePick[];
  moveCount: number;
  bytes: number;
  /** The suggestion stopped at the most one plan may move. */
  truncated: boolean;
}

export interface KeptCopy {
  id: number;
  path: string;
  name: string;
  protected: boolean;
}

export interface PlannedGroup {
  group: number;
  size: number;
  kept: KeptCopy[];
  moves: PlanItem[];
}

export interface DuplicatePlan {
  planId: number;
  /** Largest total first. */
  groups: PlannedGroup[];
  excluded: ExcludedItem[];
  totalBytes: number;
  totalFiles: number;
  needsAcknowledgement: boolean;
}

export interface DuplicateCleanupResult {
  /** `node` is the file id. */
  results: CleanupItemResult[];
  moved: number;
  alreadyGone: number;
  failed: number;
  movedBytes: number;
  /** False if the moved files could not be written to the cleanup log. */
  logSaved: boolean;
  remainingGroups: number;
  remainingReclaimableBytes: number;
}

const SCAN_ERROR_KINDS: readonly string[] = [
  'cancelled',
  'permissionDenied',
  'notFound',
  'alreadyRunning',
  'other',
];

/** Normalises whatever `invoke` rejected with into a `ScanError`. */
export function toScanError(raw: unknown): ScanError {
  if (typeof raw === 'object' && raw !== null) {
    const { kind, message } = raw as Record<string, unknown>;
    if (
      typeof kind === 'string' &&
      SCAN_ERROR_KINDS.includes(kind) &&
      typeof message === 'string'
    ) {
      return { kind: kind as ScanErrorKind, message };
    }
  }
  return { kind: 'other', message: String(raw) };
}

export function checkFullDiskAccess(): Promise<FullDiskAccessStatus> {
  return invoke<FullDiskAccessStatus>('check_full_disk_access');
}

export function openFullDiskAccessSettings(): Promise<void> {
  return invoke<void>('open_full_disk_access_settings');
}

export async function startScan(
  root: string,
  includeCloudFolders = false,
): Promise<ScanSummary> {
  try {
    return await invoke<ScanSummary>('start_scan', { root, includeCloudFolders });
  } catch (raw) {
    throw toScanError(raw);
  }
}

export function cancelScan(): Promise<void> {
  return invoke<void>('cancel_scan');
}

export function getView(request: ViewRequest = {}): Promise<ViewResponse> {
  return invoke<ViewResponse>('get_view', { request });
}

/** Raw id for an absolute path in the current scan, or `null` if absent. */
export function findNodeByPath(path: string): Promise<number | null> {
  return invoke<number | null>('find_node_by_path', { path });
}

const CLEANUP_ERROR_KINDS: readonly string[] = [
  'noScan',
  'unknownNode',
  'tooMany',
  'stalePlan',
  'needsAcknowledgement',
  'scanRunning',
  'noDiscovery',
  'unknownCandidate',
  'discoveryRunning',
  'cancelled',
  'noDuplicateScan',
  'duplicateScanRunning',
  'lastCopy',
  'protectedFile',
  'invalidRoot',
  'permissionDenied',
  'other',
];

/** Normalises whatever `invoke` rejected with into a `CleanupError`. */
export function toCleanupError(raw: unknown): CleanupError {
  if (typeof raw === 'object' && raw !== null) {
    const { kind, message } = raw as Record<string, unknown>;
    if (
      typeof kind === 'string' &&
      CLEANUP_ERROR_KINDS.includes(kind) &&
      typeof message === 'string'
    ) {
      return { kind: kind as CleanupErrorKind, message };
    }
  }
  return { kind: 'other', message: String(raw) };
}

/** Dry run. Nothing is moved. */
export async function planCleanup(nodes: number[]): Promise<CleanupPlan> {
  try {
    return await invoke<CleanupPlan>('plan_cleanup_items', { nodes });
  } catch (raw) {
    throw toCleanupError(raw);
  }
}

/** Moves the items of a plan to the Trash. A plan can be executed once. */
export async function executeCleanup(
  planId: number,
  acknowledged: boolean,
): Promise<CleanupResult> {
  try {
    return await invoke<CleanupResult>('execute_cleanup', { planId, acknowledged });
  } catch (raw) {
    throw toCleanupError(raw);
  }
}

/** Looks for cache and log folders to clean up. Read-only. */
export async function discoverCleanup(): Promise<DiscoveryResult> {
  try {
    return await invoke<DiscoveryResult>('discover_cleanup');
  } catch (raw) {
    throw toCleanupError(raw);
  }
}

export function cancelDiscovery(): Promise<void> {
  return invoke<void>('cancel_discovery');
}

/** Dry run for the chosen candidate ids. Nothing is moved. */
export async function planCleanupCandidates(ids: number[]): Promise<CleanupPlan> {
  try {
    return await invoke<CleanupPlan>('plan_cleanup_candidates', { ids });
  } catch (raw) {
    throw toCleanupError(raw);
  }
}

/** Moves the items of a candidate plan to the Trash. A plan can be executed once. */
export async function executeCleanupCandidates(
  planId: number,
  acknowledged: boolean,
): Promise<CandidateCleanupResult> {
  try {
    return await invoke<CandidateCleanupResult>('execute_cleanup_candidates', {
      planId,
      acknowledged,
    });
  } catch (raw) {
    throw toCleanupError(raw);
  }
}

export function onCleanupProgress(
  handler: (progress: DiscoveryProgress) => void,
): Promise<UnlistenFn> {
  return listen<DiscoveryProgress>('cleanup-progress', (event) => handler(event.payload));
}

/** Finds duplicate files. Read-only. Rejects with `cancelled` if cancelled. */
export async function startDuplicateScan(
  request: DuplicateScanRequest,
): Promise<DuplicateSummary> {
  try {
    return await invoke<DuplicateSummary>('start_duplicate_scan', { request });
  } catch (raw) {
    throw toCleanupError(raw);
  }
}

export function cancelDuplicateScan(): Promise<void> {
  return invoke<void>('cancel_duplicate_scan');
}

export async function getDuplicateGroups(
  request: { sort?: DuplicateSort; offset?: number; limit?: number } = {},
): Promise<DuplicateGroupsPage> {
  try {
    return await invoke<DuplicateGroupsPage>('get_duplicate_groups', { request });
  } catch (raw) {
    throw toCleanupError(raw);
  }
}

/** One keeper per group, chosen by Rust. Nothing is selected or moved. */
export async function suggestDuplicatePicks(): Promise<SuggestedPicks> {
  try {
    return await invoke<SuggestedPicks>('suggest_duplicate_picks');
  } catch (raw) {
    throw toCleanupError(raw);
  }
}

/** Dry run for the chosen copies. Nothing is moved. */
export async function planDuplicateCleanup(
  picks: DuplicatePick[],
): Promise<DuplicatePlan> {
  try {
    return await invoke<DuplicatePlan>('plan_duplicate_cleanup', { picks });
  } catch (raw) {
    throw toCleanupError(raw);
  }
}

/** Moves the copies of a plan to the Trash. A plan can be executed once. */
export async function executeDuplicateCleanup(
  planId: number,
  acknowledged: boolean,
): Promise<DuplicateCleanupResult> {
  try {
    return await invoke<DuplicateCleanupResult>('execute_duplicate_cleanup', {
      planId,
      acknowledged,
    });
  } catch (raw) {
    throw toCleanupError(raw);
  }
}

export function onDuplicateProgress(
  handler: (progress: DuplicateProgress) => void,
): Promise<UnlistenFn> {
  return listen<DuplicateProgress>('duplicate-progress', (event) =>
    handler(event.payload),
  );
}

export function onScanProgress(
  handler: (progress: ScanProgress) => void,
): Promise<UnlistenFn> {
  return listen<ScanProgress>('scan-progress', (event) => handler(event.payload));
}

/** Opens the native folder picker. Resolves to `null` when dismissed. */
export async function pickFolder(): Promise<string | null> {
  const selected = await open({ directory: true, multiple: false });
  return typeof selected === 'string' ? selected : null;
}

/** Reveals a file or folder in Finder (macOS) / Explorer (Windows). */
export function showInFinder(path: string): Promise<void> {
  return revealItemInDir(path);
}

/** Opens a Quick Look panel for a file (macOS). Fire-and-forget. */
export function previewFile(path: string): Promise<void> {
  return invoke('preview_file', { path });
}
