import { useCallback, useEffect, useRef, useState } from 'react';
import {
  cancelDuplicateScan,
  executeDuplicateCleanup,
  getDuplicateGroups,
  onDuplicateProgress,
  planDuplicateCleanup,
  startDuplicateScan,
  suggestDuplicatePicks,
  toCleanupError,
  type CleanupError,
  type DuplicateCleanupResult,
  type DuplicateExclusions,
  type DuplicateGroup,
  type DuplicatePlan,
  type DuplicateProgress,
  type DuplicateScanRequest,
  type DuplicateSort,
  type DuplicateSummary,
} from '../ipc';
import { loadIncludeCloud, saveIncludeCloud } from './scanSettings';

export const GROUPS_PAGE_SIZE = 50;

/** Matches the default in Rust. */
export const DEFAULT_MIN_SIZE_BYTES = 1024 * 1024;

export type DuplicateScanState =
  | { status: 'idle' }
  | { status: 'scanning'; progress: DuplicateProgress | null }
  | { status: 'ready'; summary: DuplicateSummary }
  | { status: 'failed'; error: CleanupError };

export type DuplicateMoveState =
  | { step: 'closed' }
  | { step: 'planning' }
  | { step: 'preview'; plan: DuplicatePlan }
  | { step: 'moving'; plan: DuplicatePlan }
  | { step: 'done'; plan: DuplicatePlan; result: DuplicateCleanupResult }
  | { step: 'error'; error: CleanupError; plan?: DuplicatePlan };

/** What the start screen collects. */
export interface DuplicateOptions {
  scope: 'home' | 'folder';
  /** The chosen folder when `scope` is `folder`. */
  folder: string | null;
  /** Copies in these folders may be kept but are never moved. */
  protectedRoots: string[];
  minSizeBytes: number;
  exclusions: DuplicateExclusions;
  includeCloud: boolean;
}

/** Copies chosen to move, by group id, with their total size. */
export interface DuplicateSelection {
  groups: ReadonlyMap<number, ReadonlySet<number>>;
  count: number;
  bytes: number;
}

interface Listing {
  groups: DuplicateGroup[];
  totalGroups: number;
  reclaimableBytes: number;
}

const EMPTY_LISTING: Listing = { groups: [], totalGroups: 0, reclaimableBytes: 0 };
const EMPTY_SELECTION: DuplicateSelection = { groups: new Map(), count: 0, bytes: 0 };

function defaultOptions(): DuplicateOptions {
  return {
    scope: 'home',
    folder: null,
    protectedRoots: [],
    minSizeBytes: DEFAULT_MIN_SIZE_BYTES,
    exclusions: {
      packages: true,
      nodeModules: true,
      git: true,
      library: true,
      trash: true,
    },
    includeCloud: loadIncludeCloud(),
  };
}

/** The request for Rust. `null` when a folder is wanted but not chosen yet. */
export function toScanRequest(options: DuplicateOptions): DuplicateScanRequest | null {
  if (options.scope === 'folder' && !options.folder) return null;
  return {
    root: options.scope === 'folder' ? (options.folder ?? undefined) : undefined,
    protectedRoots: options.protectedRoots,
    minSizeBytes: options.minSizeBytes,
    includeCloudFolders: options.includeCloud,
    exclusions: options.exclusions,
  };
}

/**
 * Duplicate finder. `start` only reads. The user, or `selectSuggested`, picks
 * the copies to move; `begin` turns that into a dry-run plan and `confirm`
 * executes exactly that plan. Only ids are sent to Rust, never paths, and
 * Rust refuses any selection that would move every copy of a file.
 */
export function useDuplicates() {
  const [options, setOptionsState] = useState(defaultOptions);
  const [scan, setScan] = useState<DuplicateScanState>({ status: 'idle' });
  const [listing, setListing] = useState<Listing>(EMPTY_LISTING);
  const [sort, setSortState] = useState<DuplicateSort>('wastedSpace');
  const [loadingGroups, setLoadingGroups] = useState(false);
  const [selection, setSelection] = useState<DuplicateSelection>(EMPTY_SELECTION);
  const [suggestionTruncated, setSuggestionTruncated] = useState(false);
  const [move, setMove] = useState<DuplicateMoveState>({ step: 'closed' });
  const scanning = useRef(false);
  const busy = useRef(false);
  // Ignores a page that arrives after a newer request was made.
  const listRequest = useRef(0);

  useEffect(() => {
    const pending = onDuplicateProgress((progress) => {
      setScan((prev) => (prev.status === 'scanning' ? { ...prev, progress } : prev));
    });
    return () => {
      void pending.then((unlisten) => unlisten());
    };
  }, []);

  const setOptions = useCallback((patch: Partial<DuplicateOptions>) => {
    if (patch.includeCloud !== undefined) saveIncludeCloud(patch.includeCloud);
    setOptionsState((prev) => ({ ...prev, ...patch }));
  }, []);

  /** Loads groups from `offset`, replacing the list when `offset` is 0. */
  const loadGroups = useCallback(
    async (order: DuplicateSort, offset: number, limit = GROUPS_PAGE_SIZE) => {
      const request = ++listRequest.current;
      setLoadingGroups(true);
      try {
        const page = await getDuplicateGroups({ sort: order, offset, limit });
        if (request !== listRequest.current) return;
        setListing((prev) => ({
          groups: offset === 0 ? page.groups : [...prev.groups, ...page.groups],
          totalGroups: page.totalGroups,
          reclaimableBytes: page.reclaimableBytes,
        }));
      } catch (raw) {
        if (request !== listRequest.current) return;
        setScan({ status: 'failed', error: toCleanupError(raw) });
      } finally {
        if (request === listRequest.current) setLoadingGroups(false);
      }
    },
    [],
  );

  const start = useCallback(async () => {
    const request = toScanRequest(options);
    if (!request || scanning.current || busy.current) return;
    scanning.current = true;
    // Ids belong to one scan, so a new scan starts with nothing chosen.
    listRequest.current++;
    setListing(EMPTY_LISTING);
    setSelection(EMPTY_SELECTION);
    setSuggestionTruncated(false);
    setMove({ step: 'closed' });
    setScan({ status: 'scanning', progress: null });
    try {
      const summary = await startDuplicateScan(request);
      setScan({ status: 'ready', summary });
      await loadGroups(sort, 0);
    } catch (raw) {
      setScan({ status: 'failed', error: toCleanupError(raw) });
    } finally {
      scanning.current = false;
    }
  }, [loadGroups, options, sort]);

  const cancelScan = useCallback(() => {
    void cancelDuplicateScan();
  }, []);

  /** Back to the start screen. Results stay in Rust until the next scan. */
  const reset = useCallback(() => {
    if (scanning.current || busy.current) return;
    listRequest.current++;
    setScan({ status: 'idle' });
    setListing(EMPTY_LISTING);
    setSelection(EMPTY_SELECTION);
    setSuggestionTruncated(false);
    setMove({ step: 'closed' });
  }, []);

  const setSort = useCallback(
    (order: DuplicateSort) => {
      setSortState(order);
      if (scan.status === 'ready') void loadGroups(order, 0);
    },
    [loadGroups, scan.status],
  );

  const loadMore = useCallback(() => {
    if (loadingGroups || listing.groups.length >= listing.totalGroups) return;
    void loadGroups(sort, listing.groups.length);
  }, [listing, loadGroups, loadingGroups, sort]);

  /** Chooses or unchooses one copy. `size` is the size of each copy in the group. */
  const toggle = useCallback(
    (groupId: number, fileId: number, size: number, checked: boolean) => {
      setSelection((prev) => {
        const files = new Set(prev.groups.get(groupId));
        if (checked === files.has(fileId)) return prev;
        if (checked) files.add(fileId);
        else files.delete(fileId);
        const groups = new Map(prev.groups);
        if (files.size === 0) groups.delete(groupId);
        else groups.set(groupId, files);
        return {
          groups,
          count: prev.count + (checked ? 1 : -1),
          bytes: Math.max(0, prev.bytes + (checked ? size : -size)),
        };
      });
    },
    [],
  );

  const clearSelection = useCallback(() => {
    setSelection(EMPTY_SELECTION);
    setSuggestionTruncated(false);
  }, []);

  /** Replaces the selection with Rust's suggestion: one keeper per group. */
  const selectSuggested = useCallback(async () => {
    if (scan.status !== 'ready' || busy.current) return;
    try {
      const suggested = await suggestDuplicatePicks();
      setSelection({
        groups: new Map(suggested.picks.map((p) => [p.group, new Set(p.files)])),
        count: suggested.moveCount,
        bytes: suggested.bytes,
      });
      setSuggestionTruncated(suggested.truncated);
    } catch (raw) {
      setMove({ step: 'error', error: toCleanupError(raw) });
    }
  }, [scan.status]);

  const begin = useCallback(async () => {
    if (busy.current || scanning.current || selection.count === 0) return;
    busy.current = true;
    setMove({ step: 'planning' });
    try {
      const picks = [...selection.groups].map(([group, files]) => ({
        group,
        files: [...files],
      }));
      setMove({ step: 'preview', plan: await planDuplicateCleanup(picks) });
    } catch (raw) {
      setMove({ step: 'error', error: toCleanupError(raw) });
    } finally {
      busy.current = false;
    }
  }, [selection]);

  const confirm = useCallback(
    async (acknowledged: boolean) => {
      if (busy.current || move.step !== 'preview') return;
      const { plan } = move;
      busy.current = true;
      setMove({ step: 'moving', plan });
      try {
        const result = await executeDuplicateCleanup(plan.planId, acknowledged);
        setSelection((prev) => withoutFiles(prev, result));
        setMove({ step: 'done', plan, result });
        // Rust already shrank its groups. Reload what was on screen.
        await loadGroups(sort, 0, Math.max(listing.groups.length, GROUPS_PAGE_SIZE));
      } catch (raw) {
        setMove({ step: 'error', error: toCleanupError(raw), plan });
      } finally {
        busy.current = false;
      }
    },
    [listing.groups.length, loadGroups, move, sort],
  );

  const close = useCallback(() => {
    if (busy.current) return;
    setMove({ step: 'closed' });
  }, []);

  return {
    options,
    setOptions,
    scan,
    listing,
    sort,
    loadingGroups,
    selection,
    suggestionTruncated,
    move,
    start,
    cancelScan,
    reset,
    setSort,
    loadMore,
    toggle,
    clearSelection,
    selectSuggested,
    begin,
    confirm,
    close,
  };
}

/** Drops files that left the disk from the selection. Failed ones stay chosen. */
function withoutFiles(
  selection: DuplicateSelection,
  result: DuplicateCleanupResult,
): DuplicateSelection {
  const gone = result.results.filter((r) => r.outcome !== 'failed');
  const goneIds = new Set(gone.map((r) => r.node));
  const groups = new Map<number, ReadonlySet<number>>();
  let count = 0;
  for (const [group, files] of selection.groups) {
    const left = new Set([...files].filter((id) => !goneIds.has(id)));
    if (left.size > 0) groups.set(group, left);
    count += left.size;
  }
  const goneBytes = gone.reduce((sum, r) => sum + r.size, 0);
  return {
    groups,
    count,
    bytes: count === 0 ? 0 : Math.max(0, selection.bytes - goneBytes),
  };
}

export type Duplicates = ReturnType<typeof useDuplicates>;
