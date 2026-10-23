/**
 * Typed wrappers around Tauri IPC commands.
 * Each function maps 1-to-1 to a command declared in src-tauri/src/commands/.
 * Field names match the camelCase serde output on the Rust side.
 */
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';

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
  | 'outsideHome';

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
  | 'other';

export interface CleanupError {
  kind: CleanupErrorKind;
  message: string;
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
